IMAGE ?= microsystem-dev:rust-1.97.1
DOCKER ?= docker
ARCH ?= aarch64
VALID_ARCHES := aarch64 riscv64 x86_64
ifneq ($(filter $(ARCH),$(VALID_ARCHES)),$(ARCH))
$(error ARCH must be aarch64, riscv64, or x86_64 (got $(ARCH)))
endif

RUN = $(DOCKER) run --rm -e ARCH=$(ARCH) -e MICROSYSTEM_DISK_PATH -e MICROSYSTEM_CONTAINER=1 -e RUSTUP_TOOLCHAIN=1.97.1 -v $(CURDIR):/workspace -w /workspace $(IMAGE)

.PHONY: image build run gui ssh test fsck clean

image:
	DOCKER=$(DOCKER) IMAGE=$(IMAGE) scripts/build-image.sh

build: image
	$(RUN) cargo run -p xtask -- build

run: build
	$(DOCKER) run --rm -it -p 127.0.0.1:$${SSH_PORT:-2222}:2222 -e ARCH=$(ARCH) -e MICROSYSTEM_DISK_PATH -e MICROSYSTEM_CONTAINER=1 -e RUSTUP_TOOLCHAIN=1.97.1 -v $(CURDIR):/workspace -w /workspace $(IMAGE) cargo run -p xtask -- qemu

gui: build
	$(DOCKER) run --rm -it -p 127.0.0.1:$${GUI_PORT:-5900}:5900 -p 127.0.0.1:$${SSH_PORT:-2222}:2222 -e ARCH=$(ARCH) -e MICROSYSTEM_DISK_PATH -e MICROSYSTEM_CONTAINER=1 -e GUI_PORT=$${GUI_PORT:-5900} -e SSH_PORT=$${SSH_PORT:-2222} -e RUSTUP_TOOLCHAIN=1.97.1 -v $(CURDIR):/workspace -w /workspace $(IMAGE) cargo run -p xtask -- gui

ssh:
	ssh -F /dev/null -T -p $${SSH_PORT:-2222} -i build/ssh/id_ed25519 -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new micro@127.0.0.1

test: build
	$(RUN) cargo run -p xtask -- test

fsck: build
	$(RUN) cargo run -p xtask -- fsck

clean:
	$(RUN) cargo clean
