.PHONY: build test e2e clean release build-debian build-alpine build-ubuntu

# Default target
all: build test

# Build
build:
	cargo build --release

# Unit tests
test:
	cargo test --all --no-fail-fast

# E2E tests (requires release build)
e2e: build
	chmod +x tests/e2e.sh
	./tests/e2e.sh

# Full check: unit + e2e
check-all: test e2e

# Clean
clean:
	cargo clean
	rm -f /tmp/rune_*

# Release build with optimizations
release:
	cargo build --release
	strip target/release/rune
	@echo "Binary: target/release/rune ($$(ls -lh target/release/rune | awk '{print $$5}'))"

# Docker builds
build-debian:
	docker build -f docker/Dockerfile.debian -t rune:debian .

build-alpine:
	docker build -f docker/Dockerfile.alpine -t rune:alpine .

build-ubuntu:
	docker build -f docker/Dockerfile.ubuntu -t rune:ubuntu .

# Install to ~/.local/bin
install: release
	mkdir -p ~/.local/bin
	cp target/release/rune ~/.local/bin/rune
	@echo "Installed to ~/.local/bin/rune"

# Create Concourse symlinks
install-concourse: release
	mkdir -p /opt/resource
	ln -sf $$(realpath target/release/rune) /opt/resource/check
	ln -sf $$(realpath target/release/rune) /opt/resource/in
	ln -sf $$(realpath target/release/rune) /opt/resource/out
	@echo "Concourse symlinks created in /opt/resource/"
