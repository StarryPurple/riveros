# Building
TARGET := riscv64gc-unknown-none-elf
MODE := release
KERNEL_ELF := target/$(TARGET)/$(MODE)/os
KERNEL_BIN := $(KERNEL_ELF).bin
DISASM_TMP := target/$(TARGET)/$(MODE)/asm
FS_IMG := ../user/target/$(TARGET)/$(MODE)/fs.img
APPS := ../user/src/bin/*

# BOARD
BOARD := qemu
SBI ?= rustsbi
BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin

# GUI
GUI ?= off
ifeq ($(GUI), off)
	GUI_OPTION := -display none
endif

# Building mode argument
ifeq ($(MODE), release)
	MODE_ARG := --release
endif

# KERNEL ENTRY
KERNEL_ENTRY_PA := 0x80200000

# Binutils
OBJDUMP := rust-objdump --arch-name=riscv64
OBJCOPY := rust-objcopy --binary-architecture=riscv64

# Disassembly
DISASM ?= -x

# Run usertests or usershell
TEST ?=

build: env $(KERNEL_BIN) fs-img 

env:
	(rustup target list | grep "riscv64gc-unknown-none-elf (installed)") || rustup target add $(TARGET)
	cargo install cargo-binutils
	rustup component add rust-src
	rustup component add llvm-tools-preview

$(KERNEL_BIN): kernel
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $@

fs-img: $(APPS)
	@cd ../user && make build TEST=$(TEST)
	@rm -f $(FS_IMG)
	@cd ../easy-fs-fuse && cargo run --release -- -s ../user/src/bin/ -t ../user/target/riscv64gc-unknown-none-elf/release/

$(APPS):

kernel:
	@echo Platform: $(BOARD)
	@cp src/linker-$(BOARD).ld src/linker.ld
	@cargo build --release
	@rm src/linker.ld

clean:
	@cargo clean

disasm: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) | less

disasm-vim: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) > $(DISASM_TMP)
	@nvim $(DISASM_TMP)
	@rm $(DISASM_TMP)

run: run-inner

QEMU_ARGS := -machine virt \
			 -bios $(BOOTLOADER) \
			 -serial stdio \
			 $(GUI_OPTION) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 -drive file=$(FS_IMG),if=none,format=raw,id=x0 \
			 -device virtio-blk-device,drive=x0 \
			 -device virtio-gpu-device \
			 -device virtio-keyboard-device \
			 -device virtio-mouse-device \
			 -device virtio-net-device,netdev=net0 \
			 -netdev user,id=net0 \
			 -object memory-backend-file,id=shm0,size=64M,mem-path=../backend-file/cxl.mm,share=on \
			 -device ivshmem-plain,memdev=shm0

# ── Multi-QEMU targets (Step 5: distributive shared memory) ──
# Each instance gets a unique boot-time instance ID (0, 1, 2).
# Serial output goes to separate log files.
# Instance 0 is always started first (initialises SHM).
QEMU_ARGS_NODE := -machine virt \
			 -bios $(BOOTLOADER) \
			 $(GUI_OPTION) \
			 -drive file=$(FS_IMG),if=none,format=raw,id=x0,snapshot=on \
			 -device virtio-blk-device,drive=x0 \
			 -device virtio-gpu-device \
			 -device virtio-keyboard-device \
			 -device virtio-mouse-device \
			 -device virtio-net-device,netdev=net0 \
			 -netdev user,id=net0 \
			 -object memory-backend-file,id=shm0,size=64M,mem-path=../backend-file/cxl.mm,share=on \
			 -device ivshmem-plain,memdev=shm0

LOGDIR := logs
NODE_BASE := -serial file:$(LOGDIR)/node-

KERNEL_NODE = target/$(TARGET)/$(MODE)/os

# Build user programs once (shared across all nodes).
build-user:
	@cd ../user && make build
	@cd ../easy-fs-fuse && cargo run --release -- -s ../user/src/bin/ -t ../user/target/riscv64gc-unknown-none-elf/release/

# Build separate kernel + binary for each instance.
# We save each binary under a unique name so run-multi can load the right one.
NODE_DIR := target/$(TARGET)/$(MODE)/nodes

build-node-%: build-user
	@mkdir -p $(NODE_DIR)
	@cp src/linker-$(BOARD).ld src/linker.ld
	@touch src/cxl/instance_id.rs   # force recompilation (option_env! not tracked by cargo)
	@RINGS_INSTANCE_ID=$* cargo build --release 2>&1 | tail -1
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $(NODE_DIR)/os-node-$*.bin
	@rm src/linker.ld

run-node-0: build-node-0
	@qemu-system-riscv64 $(QEMU_ARGS_NODE) \
		-device loader,file=$(NODE_DIR)/os-node-0.bin,addr=$(KERNEL_ENTRY_PA) \
		-serial mon:stdio

run-node-1: build-node-1
	@qemu-system-riscv64 $(QEMU_ARGS_NODE) \
		-device loader,file=$(NODE_DIR)/os-node-1.bin,addr=$(KERNEL_ENTRY_PA) \
		-serial mon:stdio

run-node-2: build-node-2
	@qemu-system-riscv64 $(QEMU_ARGS_NODE) \
		-device loader,file=$(NODE_DIR)/os-node-2.bin,addr=$(KERNEL_ENTRY_PA) \
		-serial mon:stdio

run-multi: build-user
	@mkdir -p $(LOGDIR) $(NODE_DIR)
	@echo "Building kernels for 3 instances..."
	@# Build each node's kernel into a separate binary
	@for id in 0 1 2; do \
		cp src/linker-$(BOARD).ld src/linker.ld && \
		RINGS_INSTANCE_ID=$$id cargo build --release 2>&1 | tail -1 && \
		$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $(NODE_DIR)/os-node-$$id.bin && \
		rm src/linker.ld; \
	done
	@echo "Starting 3 QEMU instances..."
	@for id in 0 1 2; do \
		qemu-system-riscv64 $(QEMU_ARGS_NODE) \
			-device loader,file=$(NODE_DIR)/os-node-$$id.bin,addr=$(KERNEL_ENTRY_PA) \
			$(NODE_BASE)$$id.log & \
		sleep 3; \
	done
	@echo "All 3 started, see logs/ for output."
	@wait

fdt:
	@qemu-system-riscv64 -M 128m -machine virt,dumpdtb=virt.out
	fdtdump virt.out

QEMU_NAME := qemu-system-riscv64
qemu-version-check:
	@sh scripts/qemu-ver-check.sh $(QEMU_NAME)

run-inner: qemu-version-check build
	@qemu-system-riscv64 $(QEMU_ARGS)

debug: qemu-version-check build
	@tmux new-session -d \
		"qemu-system-riscv64 $(QEMU_ARGS) -s -S" && \
		tmux split-window -h "riscv64-unknown-elf-gdb -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'" && \
		tmux -2 attach-session -d

gdbserver: qemu-version-check build
	@qemu-system-riscv64 $(QEMU_ARGS) -s -S

gdbclient:
	@riscv64-unknown-elf-gdb -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'

.PHONY: build env kernel clean disasm disasm-vim run-inner fs-img gdbserver gdbclient fdt qemu-version-check run-node-0 run-node-1 run-node-2 run-multi
