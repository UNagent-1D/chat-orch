# Multi-stage Dockerfile for the conversation-chat service (Go + Gin + MongoDB + Redis).
#
# Build context: .dev/services/conversation-chat
# Used by: docker-compose.yml → conversation-chat

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
RUN CGO_ENABLED=0 GOOS=linux go build -ldflags="-s -w" -o /conversation-chat ./cmd/server

# ── Stage 2: Runtime ─────────────────────────────────────────────────
FROM alpine:3.20
RUN apk add --no-cache ca-certificates wget

COPY --from=builder /conversation-chat /app/conversation-chat

# Non-root user
RUN addgroup -S appuser && adduser -S appuser -G appuser
USER appuser

EXPOSE 8082

HEALTHCHECK --interval=10s --timeout=3s --start-period=15s --retries=5 \
    CMD wget -qO- http://localhost:8082/api/v1/health || exit 1

ENTRYPOINT ["/app/conversation-chat"]
