DOCKER ?= docker
IMAGE ?= rune
CONTEXT ?= .

.PHONY: build-debian build-alpine build-ubuntu test clean

build-debian:
	$(DOCKER) build -f docker/Dockerfile.debian -t $(IMAGE):debian $(CONTEXT)

build-alpine:
	$(DOCKER) build -f docker/Dockerfile.alpine -t $(IMAGE):alpine $(CONTEXT)

build-ubuntu:
	$(DOCKER) build -f docker/Dockerfile.ubuntu -t $(IMAGE):ubuntu $(CONTEXT)

test:
	cargo test --locked

clean:
	cargo clean
