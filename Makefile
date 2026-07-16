# outflow — common developer tasks. Run `make` (or `make help`) to list them.
# Prereqs: Rust toolchain + Node/npm (see docs/DEVELOPMENT.md).

APP_DIR := app

# Database for dev mode. Isolated from a production deployment's DB.
# Override: make dev DB=/path.db
DB ?= $(HOME)/outflow-dev.db

.DEFAULT_GOAL := help
.PHONY: help dev web server run serve deps test check fmt lint pull clean

help: ## List available targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*## /\t/' \
		| awk -F'\t' '{printf "  make %-7s %s\n", $$1, $$2}'

dev: ## Hot-reload frontend (vite on :1420, proxying /api to :8080) — run `make serve` alongside
	cd $(APP_DIR) && npm run dev

web: ## Build the production frontend into app/dist
	cd $(APP_DIR) && npm run build

server: ## Build the release server binary
	cargo build --release -p outflow-server

run: web ## Rebuild app/dist, then serve it + /api against $(DB) — use after UI changes
	OUTFLOW_DB="$(DB)" cargo run -p outflow-server

serve: ## Serve the existing app/dist without rebuilding (backend-only iteration)
	OUTFLOW_DB="$(DB)" cargo run -p outflow-server

deps: ## Install frontend dependencies (first-time setup)
	cd $(APP_DIR) && npm install

test: ## Run the core + net test suites
	cargo test -p outflow-core -p outflow-net

check: ## Fast pre-commit gate: format check, clippy, tests
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test -p outflow-core -p outflow-net

fmt: ## Format all Rust code
	cargo fmt

lint: ## Clippy across the workspace, warnings as errors
	cargo clippy --workspace --all-targets -- -D warnings

pull: ## Ingest the offline Plaid fixture into $(DB)
	cargo run -p outflow-cli -- --db "$(DB)" pull --from-file examples/plaid-fixture.json

clean: ## Remove cargo + frontend build artifacts
	cargo clean
	rm -rf $(APP_DIR)/dist $(APP_DIR)/node_modules
