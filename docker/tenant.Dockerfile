# Multi-stage Dockerfile for the Tenant Service (Go + Gin + PostgreSQL).
#
# Build context: .dev/services/tenant
# Used by: docker-compose.yml → tenant-service

# ── Stage 1: Builder ─────────────────────────────────────────────────
FROM golang:1.25-alpine AS builder
WORKDIR /app

# Install git (needed for private module fetches, if any)
RUN apk add --no-cache git

# Cache dependency download
COPY go.mod go.sum ./
RUN go mod download

# Copy source and build
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -ldflags="-s -w" -o /tenant-service .

# ── Stage 2: Runtime ─────────────────────────────────────────────────
FROM alpine:3.20
RUN apk add --no-cache ca-certificates wget

COPY --from=builder /tenant-service /app/tenant-service

# Non-root user
RUN addgroup -S appuser && adduser -S appuser -G appuser
USER appuser

EXPOSE 8080

# No /health endpoint yet — verify TCP port is accepting connections.
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=5 \
    CMD nc -z localhost 8080 || exit 1

ENTRYPOINT ["/app/tenant-service"]
