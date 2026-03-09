-- seed.sql — Test data for the Tenant Service (PostgreSQL).
--
-- UUIDs here MUST match the hardcoded values in mock-services/tenant-service/app.py
-- and mock-services/acr-service/app.py so that all services agree on identity.
--
-- Mounted at: /docker-entrypoint-initdb.d/02_seed.sql
-- Runs ONCE on first postgres volume creation. To re-seed: docker compose down -v
--
-- Test credentials:
--   Email:    admin@test-hospital.com
--   Password: admin123

-- ── Tenant ───────────────────────────────────────────────────────────
INSERT INTO tenants (id, name, domain, is_active)
VALUES (
    '550e8400-e29b-41d4-a716-446655440000',
    'Test Hospital',
    'test-hospital.local',
    true
) ON CONFLICT (id) DO NOTHING;

-- ── App Admin user (no tenant — global administrator) ────────────────
-- Password: admin123 → bcrypt hash (cost 10)
INSERT INTO users (id, email, password_hash, first_name, last_name, is_active)
VALUES (
    '660e8400-e29b-41d4-a716-446655440001',
    'admin@platform.local',
    '$2b$10$gccAgngfaWYjd8krHfCvDu/xpMb.lc3MgbyKr7NvTna9ZXjS8jLQm',
    'Platform',
    'Admin',
    true
) ON CONFLICT (id) DO NOTHING;

INSERT INTO user_tenants (user_id, tenant_id, role)
VALUES (
    '660e8400-e29b-41d4-a716-446655440001',
    NULL,
    'app_admin'
) ON CONFLICT (user_id, tenant_id) DO NOTHING;

-- ── Tenant Admin user ────────────────────────────────────────────────
-- Password: admin123 → bcrypt hash (cost 10)
INSERT INTO users (id, email, password_hash, first_name, last_name, is_active)
VALUES (
    '660e8400-e29b-41d4-a716-446655440002',
    'admin@test-hospital.com',
    '$2b$10$gccAgngfaWYjd8krHfCvDu/xpMb.lc3MgbyKr7NvTna9ZXjS8jLQm',
    'Hospital',
    'Admin',
    true
) ON CONFLICT (id) DO NOTHING;

INSERT INTO user_tenants (user_id, tenant_id, role)
VALUES (
    '660e8400-e29b-41d4-a716-446655440002',
    '550e8400-e29b-41d4-a716-446655440000',
    'tenant_admin'
) ON CONFLICT (user_id, tenant_id) DO NOTHING;

-- ── Tenant Operator user ─────────────────────────────────────────────
-- Password: admin123 → bcrypt hash (cost 10)
INSERT INTO users (id, email, password_hash, first_name, last_name, is_active)
VALUES (
    '660e8400-e29b-41d4-a716-446655440003',
    'operator@test-hospital.com',
    '$2b$10$gccAgngfaWYjd8krHfCvDu/xpMb.lc3MgbyKr7NvTna9ZXjS8jLQm',
    'Hospital',
    'Operator',
    true
) ON CONFLICT (id) DO NOTHING;

INSERT INTO user_tenants (user_id, tenant_id, role)
VALUES (
    '660e8400-e29b-41d4-a716-446655440003',
    '550e8400-e29b-41d4-a716-446655440000',
    'tenant_operator'
) ON CONFLICT (user_id, tenant_id) DO NOTHING;
