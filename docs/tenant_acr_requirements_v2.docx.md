  
**REQUIREMENTS SPECIFICATION**

Tenant Service  &  Agent Config Registry

*Prototype v1.0  |  March 2026*

| Document Purpose This document defines the functional and technical requirements for the first two core services of the multi-tenant conversational AI platform: the Tenant Service (source of truth for tenant identity and configuration) and the Agent Config Registry (versioned runtime configuration for AI agents). Both services are scoped for the MVP prototype. |
| :---- |

# **1\. System Context**

These two services sit inside the Tenant boundary of the overall platform architecture. The General Orchestrator delegates to them to resolve who is calling (Tenant Service) and how the agent should behave (Agent Config Registry). Both services expose REST APIs consumed by internal platform components only — never directly by end users.

## **1.1 Architectural Position**

| Component | Role in the system |
| :---- | :---- |
| General Orchestrator | Top-level router; calls Tenant Service to authenticate tenant on every request |
| Tenant Service | Source of truth — identity, plan, channel keys, data source pointers |
| Agent Config Registry | Versioned runtime config — tools, model policy, flow rules per agent profile |
| Stats / Analytics | Receives events from the Orchestrator; NOT owned by either service here |
| NOSQL / Audit | Stores raw conversation data; NOT owned by either service here |

## **1.2 Tech Stack Decisions**

| Decision | Choice & Rationale |
| :---- | :---- |
| Language / Framework | Go \+ Gin — minimal latency, straightforward REST, small memory footprint |
| Database | PostgreSQL (relational) — one schema per tenant for hard data isolation |
| LLM SDK | OpenAI Go SDK (first prototype); abstracted behind an interface for future swap |
| Auth model | JWT with role claims: app\_admin, tenant\_admin, tenant\_operator |
| Prototype scope | Agent Config stored inside Tenant DB; split into its own service in v2 |

# **2\. Tenant Service**

|   Service 1 of 2  —  Source of Truth for Tenant Identity & Configuration |
| :---- |

The Tenant Service is the authoritative record for everything that describes a client organisation (tenant) and how they connect to the platform. For this prototype the tenant is a hospital whose core use case is scheduling patient appointments. All other services treat this service as read-only reference data.

## **2.1 Responsibilities**

### **What it OWNS**

* Client identity: legal name, slug/handle, plan tier, account status (active / suspended / churned), branding tokens (logo URL, primary colour hex).

* Channel configuration: WhatsApp Business number \+ API key reference, web-widget embed key, webhook secret references (stored as Vault key names, not raw secrets).

* Agent business profiles: named profiles per tenant (e.g. 'Scheduling Bot') that define scheduling flow rules, escalation rules, allowed specialties, and allowed locations. The problem domain (what the bot is for) lives here, NOT in the users table.

* External data source pointers: base URL and per-operation route map for the hospital mock API (doctors list, schedules, appointment booking). See data\_sources design below.

* User roster: the three user roles scoped to this tenant. The users table records who can log in and what they can do. It does NOT describe the business domain.

### **What it does NOT OWN**

* Raw conversation messages or tool call traces — owned by the MongoDB/NoSQL layer.

* High-volume metrics or quality stats — owned by the Stats/Analytics service.

* LLM model parameters, tool permissions, or flow versioning — owned by Agent Config Registry.

* Actual secrets or credentials — the service stores Vault key names only.

* The problem domain of a user (e.g. 'this operator handles cardiology') — that belongs to agent\_profiles, not users.

## **2.2 Data Model**

Each tenant gets its own PostgreSQL schema (schema-per-tenant isolation). A global schema holds the tenant registry and app-admin users.

### **Global Schema — tenants table**

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK | Generated on creation |
| slug | TEXT UNIQUE NOT NULL | URL-safe identifier, e.g. hospital-san-ignacio |
| name | TEXT NOT NULL | Display name |
| plan | ENUM(free, starter, pro, enterprise) | Determines feature flags |
| status | ENUM(active, suspended, churned) | Lifecycle state |
| branding\_logo\_url | TEXT | CDN URL, nullable |
| branding\_primary\_color | CHAR(7) | Hex e.g. \#2E75B6, nullable |
| created\_at / updated\_at | TIMESTAMPTZ | Audit timestamps |

### **Global Schema — users table**

The users table answers one question only: who is allowed to log in, and what can they do? It is intentionally minimal. Domain knowledge (which specialties an operator handles) belongs in agent\_profiles, not here.

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK |  |
| email | TEXT UNIQUE NOT NULL | Login credential |
| password\_hash | TEXT NOT NULL | bcrypt, cost factor 12 |
| role | ENUM(app\_admin, tenant\_admin, tenant\_operator) | Controls API access level |
| tenant\_id | UUID NULLABLE FK \-\> tenants.id | NULL for app\_admin; required for all other roles |
| is\_active | BOOLEAN DEFAULT true | Deactivated users cannot log in |
| created\_at / updated\_at | TIMESTAMPTZ | Audit timestamps |

**Why the problem domain does NOT go in users:**

* Users can change roles or be reassigned. Business rules should not be coupled to a user record.

* Multiple operators may share the same flow rules. Centralising rules in agent\_profiles avoids duplication.

* MongoDB owns conversation runtime data per session. If you need to track which operator handled a conversation, store user\_id in the MongoDB document, not in the users table here.

### **Per-Tenant Schema — channels table**

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK |  |
| tenant\_id | UUID FK | Reference to global tenants.id |
| channel\_type | ENUM(whatsapp, web\_widget) |  |
| channel\_key | TEXT NOT NULL | Phone number or embed key |
| webhook\_secret\_ref | TEXT | Vault key name, NOT the secret value |
| is\_active | BOOLEAN DEFAULT true |  |

### **Per-Tenant Schema — agent\_profiles table**

This is where the problem domain lives. Each profile describes a specific bot behaviour: what it is allowed to do and in what context. For the hospital prototype there will be one profile: the Scheduling Bot.

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK |  |
| name | TEXT NOT NULL | e.g. Scheduling Bot, Triage Bot |
| description | TEXT | Plain-language description of the problem this profile solves |
| scheduling\_flow\_rules | JSONB | Step-by-step flow the bot follows to book an appointment |
| escalation\_rules | JSONB | Conditions that trigger handoff to a human operator |
| allowed\_specialties | TEXT\[\] | e.g. {cardiology, pediatrics, general} |
| allowed\_locations | TEXT\[\] | e.g. {bogota-norte, medellin-centro} |
| agent\_config\_id | UUID FK \-\> agent\_configs.id | Points to the active runtime config (LLM params, tools) |

### **Per-Tenant Schema — data\_sources table**

Stores the base URL for each external system the agent can call. For the hospital mock API, one row will represent the scheduling system.

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK |  |
| name | TEXT NOT NULL | Human label e.g. Hospital Mock API |
| source\_type | ENUM(scheduling, patient\_registry) | Category of the external system |
| base\_url | TEXT NOT NULL | Root URL e.g. https://mock-hospital-api.internal |
| credential\_ref | TEXT | Vault key name for auth token (plain string in MVP) |
| route\_configs | JSONB NOT NULL | Map of operations to HTTP method \+ path (see below) |
| is\_active | BOOLEAN DEFAULT true |  |

**The route\_configs column — solving the GET/POST/PATCH problem:**

Storing only a base URL is not enough because the Orchestrator needs to know which HTTP method and path to use for each operation. The route\_configs JSONB column stores a map of logical operation names to their HTTP method and relative path. This gives the platform full control over read vs. write operations without hard-coding anything in the Orchestrator code.

| {   "list\_doctors":           { "method": "GET",    "path": "/doctors" },   "get\_doctor\_schedule":    { "method": "GET",    "path": "/doctors/{id}/schedule" },   "book\_appointment":       { "method": "POST",   "path": "/appointments" },   "reschedule\_appointment": { "method": "PATCH",  "path": "/appointments/{id}" },   "cancel\_appointment":     { "method": "DELETE", "path": "/appointments/{id}" } } |
| :---- |

**How the Orchestrator uses this:**

* When the LLM decides to call list\_doctors, the Orchestrator reads the route\_configs entry and constructs GET https://mock-hospital-api.internal/doctors.

* When the LLM decides to call book\_appointment, it constructs POST https://mock-hospital-api.internal/appointments with the body from the LLM tool call.

* Adding a new operation requires only a new entry in route\_configs — no code change in the Orchestrator.

* Read vs. write is explicit in the method field. The Orchestrator can enforce read-only mode for tenant\_operators by rejecting tool calls whose method is not GET.

## **2.3 API Endpoints (MVP)**

Base path: /api/v1. All endpoints require a valid JWT except health check.

| Method \+ Path | Required Role | Description |
| :---- | :---- | :---- |
| GET /tenants | app\_admin | List all tenants (paginated) |
| POST /tenants | app\_admin | Create a new tenant \+ provision its DB schema |
| GET /tenants/:id | app\_admin, tenant\_admin (own) | Get tenant detail |
| PATCH /tenants/:id | app\_admin, tenant\_admin (own) | Update name, plan, status, branding |
| GET /tenants/:id/channels | tenant\_admin | List channel configs |
| POST /tenants/:id/channels | tenant\_admin | Add a channel |
| PATCH /tenants/:id/channels/:cid | tenant\_admin | Update / deactivate a channel |
| GET /tenants/:id/profiles | tenant\_admin, tenant\_operator | List agent profiles (incl. allowed specialties/locations) |
| POST /tenants/:id/profiles | tenant\_admin | Create an agent profile |
| PATCH /tenants/:id/profiles/:pid | tenant\_admin | Update profile rules, specialties, locations |
| GET /tenants/:id/data-sources | tenant\_admin | List external data source pointers \+ route configs |
| POST /tenants/:id/data-sources | tenant\_admin | Add a data source with route\_configs map |
| PATCH /tenants/:id/data-sources/:did | tenant\_admin | Update route\_configs (add/remove operations) |
| GET /users | app\_admin | List all users across all tenants |
| POST /users | app\_admin, tenant\_admin | Create a user (tenant\_admin constrained to own tenant) |
| PATCH /users/:uid | app\_admin, tenant\_admin (own tenant) | Update role or deactivate |
| GET /health | none | Liveness / readiness probe |

## **2.4 Access Control Rules**

| Role | Scope | Capabilities |
| :---- | :---- | :---- |
| app\_admin | Global — all tenants | Full CRUD on tenants, channels, profiles, data sources, users. Can see all data across all tenants. |
| tenant\_admin | Own tenant only | Read/write on own tenant config, channels, profiles, data sources. Can create and manage tenant\_operator users within own tenant only. |
| tenant\_operator | Own tenant only | Read-only on own tenant profiles and channel list. Cannot modify any configuration. |

**Key enforcement rules:**

* A tenant\_admin MUST have tenant\_id set; requests targeting a different tenant\_id return 403\.

* A tenant\_operator cannot call any write endpoint (POST, PATCH, DELETE) — all return 403\.

* app\_admin cannot be scoped to a tenant\_id (must be NULL).

* Password reset and MFA are out of scope for MVP.

# **3\. Agent Config Registry**

|   Service 2 of 2  —  Versioned Runtime Configuration for AI Agents |
| :---- |

The Agent Config Registry (ACR) owns everything that defines how an AI agent behaves at runtime. In the MVP the ACR shares the Tenant PostgreSQL database; it will split into its own service in v2 when versioning and approval workflows are needed.

## **3.1 How ACR Relates to the Tenant Service — The Key Distinction**

This is the most important conceptual boundary in the system. Both services have tables that describe an agent, but they answer completely different questions.

|  | agent\_profiles (Tenant Service) | agent\_configs (ACR) |
| :---- | :---- | :---- |
| **Question it answers** | What is this bot for, and what is it allowed to do in the real world? | How should the AI model behave technically to fulfil that purpose? |
| **Owned by** | Tenant Service | Agent Config Registry |
| **Example data** | Name: Scheduling Bot. Allowed specialties: cardiology. Locations: Bogota. | Model: gpt-4o. Temperature: 0.3. Tools: list\_doctors, book\_appointment. |
| **Who changes it** | Hospital admin (business decision) | Technical admin (AI / prompt engineering decision) |
| **Changes often?** | Rarely — business rules are stable | More frequently — prompts and model params are tuned |
| **Relationship** | One profile points to one active config via agent\_config\_id FK | One config version is active per profile; history is retained |

In short: agent\_profiles is the business contract ('a cardiology scheduling bot in Bogota'). agent\_configs is the technical implementation of that contract ('use gpt-4o at 0.3 temperature with these 3 tools'). You can retune the model without touching the business profile, and you can add a new specialty without touching the LLM params.

## **3.2 Responsibilities**

### **What it OWNS**

* Versioned agent configs (one active version per profile, history retained):

* Conversation policy — flow rules governing how the agent handles a conversation turn.

* Escalation rules — conditions that trigger handoff to a human operator.

* Tool permissions — which tools are enabled and any parameter constraints (e.g. max booking horizon in days).

* LLM parameters — model name, temperature, max tokens, system prompt text.

* Channel formatting rules — per-channel response constraints (e.g. WhatsApp max 1600 chars).

* Config validation before activation: schema checks and guardrails (temperature 0-2, tools must exist in registry).

* Draft / active / archived lifecycle for configs.

### **What it does NOT OWN**

* Tenant identity, plan, or billing data — Tenant Service owns this.

* Runtime metrics, call traces, or logs — Stats / Audit layer owns this.

* Raw conversation messages — MongoDB/NoSQL layer owns this.

* Business rules about allowed specialties or locations — that lives in agent\_profiles (Tenant Service).

## **3.3 Data Model**

### **agent\_configs table**

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK |  |
| agent\_profile\_id | UUID FK \-\> agent\_profiles.id | Which profile this config version serves |
| version | INTEGER NOT NULL | Monotonically increasing per profile (1, 2, 3...) |
| status | ENUM(draft, active, archived) | Only one active per profile at a time |
| conversation\_policy | JSONB NOT NULL | Step-by-step flow rules for the LLM loop |
| escalation\_rules | JSONB NOT NULL | Handoff trigger conditions (e.g. user frustrated 3x) |
| tool\_permissions | JSONB NOT NULL | Array of { tool\_name, constraints } objects |
| llm\_params | JSONB NOT NULL | model, temperature, max\_tokens, system\_prompt |
| channel\_format\_rules | JSONB | Per-channel overrides e.g. { whatsapp: { max\_chars: 1600 } } |
| created\_by | UUID FK \-\> users.id | Who created this config version |
| created\_at / activated\_at | TIMESTAMPTZ | Timestamps for creation and activation |

JSONB example for llm\_params:

| {   "model": "gpt-4o",   "temperature": 0.3,   "max\_tokens": 1024,   "system\_prompt": "You are a scheduling assistant for Hospital X..." } |
| :---- |

### **tool\_registry table (global schema)**

A global catalog of every tool the platform supports. Tenant admin selects which subset to enable in their agent\_config tool\_permissions.

| Column | Type | Notes |
| :---- | :---- | :---- |
| id | UUID PK |  |
| name | TEXT UNIQUE NOT NULL | Machine name e.g. list\_doctors, book\_appointment |
| description | TEXT | Human-readable description shown in admin UI |
| openai\_function\_def | JSONB NOT NULL | Full OpenAI function-calling JSON definition |
| is\_active | BOOLEAN DEFAULT true | app\_admin can globally disable a tool |

**Hospital prototype — initial tool registry entries:**

| Tool name | What it does |
| :---- | :---- |
| list\_doctors | GET /doctors — returns available doctors and their specialties |
| get\_doctor\_schedule | GET /doctors/{id}/schedule — returns available slots for a doctor |
| book\_appointment | POST /appointments — creates a new appointment (write operation) |
| reschedule\_appointment | PATCH /appointments/{id} — moves an existing appointment |
| cancel\_appointment | DELETE /appointments/{id} — cancels an appointment |

## **3.4 API Endpoints (MVP)**

| Method \+ Path | Required Role | Description |
| :---- | :---- | :---- |
| GET /tenants/:id/profiles/:pid/configs | tenant\_admin, tenant\_operator | List all config versions for a profile |
| GET /tenants/:id/profiles/:pid/configs/active | tenant\_admin, tenant\_operator | Get the currently active config |
| POST /tenants/:id/profiles/:pid/configs | tenant\_admin | Create a new draft config version |
| PATCH /tenants/:id/profiles/:pid/configs/:cid | tenant\_admin | Update a draft config (validation runs on save) |
| POST /tenants/:id/profiles/:pid/configs/:cid/activate | tenant\_admin | Promote draft to active; previous version archived atomically |
| GET /tool-registry | app\_admin, tenant\_admin | List all available tools in the global catalog |
| POST /tool-registry | app\_admin | Register a new tool with its OpenAI function definition |
| PATCH /tool-registry/:tid | app\_admin | Update description or deactivate a tool globally |

## **3.5 Validation Rules**

| Rule | Detail |
| :---- | :---- |
| model must be known | Allowed list: gpt-4o, gpt-4o-mini (expandable via config file). Unknown strings are rejected with 400\. |
| temperature range | Must be a float in \[0.0, 2.0\]. |
| max\_tokens range | Must be integer in \[1, 4096\] for MVP. |
| tool\_permissions | Each tool name must exist in tool\_registry with is\_active=true. Unknown or disabled tools are rejected. |
| Only one active config per profile | POST /activate atomically archives the current active version in a single DB transaction. |
| escalation\_rules non-empty | At least one rule must be defined to prevent runaway agents. |
| channel\_format\_rules | If present, max\_chars per channel must be a positive integer. |

# **4\. Cross-Cutting Requirements**

## **4.1 Database Isolation Strategy**

Each tenant gets a dedicated PostgreSQL schema named tenant\_\<slug\>. The Tenant Service provisions this schema atomically on tenant creation.

* Global schema holds: tenants, users, tool\_registry.

* Per-tenant schemas hold: channels, agent\_profiles, agent\_configs, data\_sources.

* DB connection sets search\_path=tenant\_\<slug\>,public so queries are schema-aware without per-query prefixing.

* Single DB instance for the prototype; connection pooling via pgxpool (Go pgx driver).

## **4.2 Error Handling**

| HTTP Status | When to use |
| :---- | :---- |
| 400 Bad Request | Validation failure (invalid JSON, field out of range, unknown tool). Body must include an error field with a human-readable message. |
| 401 Unauthorized | Missing or invalid JWT. |
| 403 Forbidden | Valid JWT but insufficient role or wrong tenant scope. |
| 404 Not Found | Resource does not exist or belongs to a different tenant. |
| 409 Conflict | Unique constraint violation (e.g. duplicate channel key). |
| 500 Internal Server Error | Unexpected DB or runtime error. Log full detail server-side; return a generic message to the client. |

## **4.3 Logging & Observability (MVP)**

* Structured JSON logs — all lines include tenant\_id, request\_id, user\_id, latency\_ms.

* Request ID injected via X-Request-ID header (generated if absent).

* No distributed tracing in MVP; add OpenTelemetry in v2.

* Health endpoint returns 200 with JSON body { status: ok, db: ok/error }.

## **4.4 Out of Scope for MVP**

* Multi-version approval workflows (draft \-\> review \-\> approved \-\> active).

* Webhook delivery for config change events.

* Rate limiting per tenant.

* Real Vault integration (credential\_ref stored as plain string in MVP).

* Payment and billing integration.

# **5\. Open Questions & Decisions Needed**

Each item below is a real technical decision the team needs to align on. The problem and options are explained plainly.

## **Q1 — Schema Migration Strategy**

| The problem |
| :---- |

When the codebase needs to change a database table (add a column, rename a field), we need a controlled way to apply that change to every tenant schema in production without manual SQL. Two mature Go tools exist for this.

| Option | What it means in practice |
| :---- | :---- |
| golang-migrate | Simple file-based migrations (001\_create\_tenants.up.sql, .down.sql). Easy to understand, battle-tested. The migration runner must loop over all tenant schemas and apply the same migration to each. Recommended for MVP. |
| Atlas | Schema-as-code: you describe the desired final state and Atlas computes the diff. More powerful but a steeper learning curve. Better for v2 when schemas may diverge across tenants. |

## **Q2 — Vault Integration Timeline**

| The problem |
| :---- |

Currently, channel webhook secrets and API credentials are stored as plain strings in the database. This is fine for a local prototype but is a security risk in any shared or production environment. Vault (HashiCorp or AWS Secrets Manager) would store the actual secret values; the DB would hold only a key name to look up at runtime. Decision needed: when do we add real Vault? Recommendation: v1.1 — after the demo is validated but before any real hospital data touches the system.

## **Q3 — System Prompt Storage**

| The problem |
| :---- |

The LLM needs a system prompt that sets the agent's persona and rules (e.g. 'You are a scheduling assistant for Hospital X. Never discuss topics outside appointments.'). Two storage options exist:

| Option | Trade-offs |
| :---- | :---- |
| Inline in llm\_params JSONB (recommended for MVP) | Simple. The full prompt text sits in the database column alongside model and temperature. Easy to read and version. Can grow large (500-2000 tokens typical). |
| Reference to an external store (v2) | The DB stores a key like prompts/scheduling-v2 and the text lives in S3 or a CMS. Cleaner for long prompts, supports non-engineer editing. Adds an extra network call per agent startup. |

## **Q4 — Tenant Slug Uniqueness & Reserved Names**

| The problem |
| :---- |

Every tenant slug is also used as the PostgreSQL schema name. Slugs must not collide with built-in PostgreSQL schema names (public, pg\_catalog, information\_schema) which already exist and cannot be overwritten. Recommendation: enforce uniqueness via a DB unique index on tenants.slug, plus a short server-side blocklist of reserved PostgreSQL schema names. No complex reserved-word list is needed for MVP.

## **Q5 — LLM SDK Abstraction Layer**

| The problem |
| :---- |

The prototype uses the OpenAI Go SDK directly. If the team later wants to switch to Anthropic Claude, Google Gemini, or a self-hosted model, every place in the code that calls the OpenAI SDK would need to be changed. Recommendation: define a small Go interface (e.g. LLMClient) with methods like Complete and StreamComplete from day one. The OpenAI SDK becomes one implementation of that interface. Swapping providers later means writing a new implementation with zero handler code changes. This costs roughly two hours of extra work upfront and saves days later.

## **Q6 — API Versioning & Deprecation Policy**

| The problem |
| :---- |

All routes are prefixed /api/v1. When we make a breaking change we can introduce /api/v2 without breaking existing clients. Decision needed: agree on the deprecation policy before v2. Recommendation: v1 endpoints stay alive for a minimum of 3 months after v2 launches, with a Sunset response header added to deprecated endpoints so clients know when to upgrade.

# **6\. Summary**

| Attribute | Tenant Service | Agent Config Registry |
| :---- | :---- | :---- |
| **Primary concern** | Who is the tenant, how are they set up, what is the business domain | How should the agent technically behave at runtime |
| **DB location** | Per-tenant PostgreSQL schema | Same per-tenant schema (MVP co-location) |
| **Key tables** | tenants, channels, agent\_profiles, users, data\_sources | agent\_configs, tool\_registry (global) |
| **Write roles** | app\_admin, tenant\_admin | tenant\_admin only |
| **Read roles** | All 3 roles (scoped by role) | All 3 roles (scoped by role) |
| **Stack** | Go \+ Gin \+ pgx | Go \+ Gin \+ pgx \+ OpenAI SDK |
| **MVP simplification** | Single profile per tenant; route\_configs in data\_sources | No approval workflow; draft goes directly to active |

*Both services share the same Go module and binary in the MVP, exposed as separate route groups (/tenants and /agent-configs). They will be split into independent deployable services when the team is ready to manage separate deployments.*