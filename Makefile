# Convenience targets wrapping scripts/bootstrap.sh (see README).
.PHONY: deps build build-backend run help

help:
	@echo "Targets:"
	@echo "  deps           system packages + cargo check (no compile)"
	@echo "  build          release binary; rebuild UI if dist missing"
	@echo "  build-backend  release binary only (--skip-frontend)"
	@echo "  run            sudo ./target/release/pack"

deps:
	@bash scripts/bootstrap.sh --deps-only

build:
	@bash scripts/bootstrap.sh --no-apt

build-backend:
	@bash scripts/bootstrap.sh --no-apt --skip-frontend

run:
	@sudo ./target/release/pack
