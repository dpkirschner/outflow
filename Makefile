# outflow — common developer tasks. Run `make` (or `make help`) to list them.
# Prereqs: Rust toolchain + Node/npm (see docs/DEVELOPMENT.md).

APP_DIR := app
BUNDLE  := $(APP_DIR)/src-tauri/target/release/bundle/macos/outflow.app
DEST    := /Applications/outflow.app

# Database for dev mode. Dev runs PLAINTEXT and can't open the installed app's
# encrypted DB, so it uses its own file — isolated from your installed app. It
# auto-populates from your connected bank on launch. Override: make dev DB=/path.db
DB ?= $(HOME)/outflow-dev.db

.DEFAULT_GOAL := help
.PHONY: help dev app deps test

help: ## List available targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*## /\t/' \
		| awk -F'\t' '{printf "  make %-6s %s\n", $$1, $$2}'

dev: ## Start the app in dev mode (hot-reload) against $(DB)
	cd $(APP_DIR) && OUTFLOW_DB="$(DB)" npm run tauri dev

app: ## Build the encrypted .app and install it to /Applications
	cd $(APP_DIR) && npm run bundle
	rm -rf "$(DEST)"
	cp -R "$(BUNDLE)" "$(DEST)"
	@echo "Installed $(DEST) — open it from Launchpad."

deps: ## Install frontend dependencies (first-time setup)
	cd $(APP_DIR) && npm install

test: ## Run the core + net test suites
	cargo test -p outflow-core -p outflow-net
