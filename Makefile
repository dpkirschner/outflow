# outflow — common developer tasks. Run `make` (or `make help`) to list them.
# Prereqs: Rust toolchain + Node/npm (see docs/DEVELOPMENT.md).

APP_DIR := app

# Database for dev mode. Isolated from a production deployment's DB.
# Override: make dev DB=/path.db
DB ?= $(HOME)/outflow-dev.db

.DEFAULT_GOAL := help
.PHONY: help dev web server run deps test

help: ## List available targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*## /\t/' \
		| awk -F'\t' '{printf "  make %-6s %s\n", $$1, $$2}'

dev: ## Hot-reload frontend (vite on :1420, proxying /api to :8080) — run `make run` alongside
	cd $(APP_DIR) && npm run dev

web: ## Build the production frontend into app/dist
	cd $(APP_DIR) && npm run build

server: ## Build the release server binary
	cargo build --release -p outflow-server

run: ## Run the server against $(DB), serving app/dist
	OUTFLOW_DB="$(DB)" cargo run -p outflow-server

deps: ## Install frontend dependencies (first-time setup)
	cd $(APP_DIR) && npm install

test: ## Run the core + net test suites
	cargo test -p outflow-core -p outflow-net
