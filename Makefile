.PHONY: build run dev-ui check

build:
	cd frontend && npm ci && npm run build
	cargo build --manifest-path backend/Cargo.toml

run:
	cargo run --manifest-path backend/Cargo.toml

dev-ui:
	cd frontend && npm run dev

check:
	cargo test --manifest-path backend/Cargo.toml
	cd frontend && npm run build
