TARGET := x86_64-unknown-linux-musl
BIN := target/$(TARGET)/release/souris-facile-pingouin
VERSION = $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TARBALL = build/souris-facile-pingouin-$(VERSION)-x86_64.tar.gz
PREFIX ?= $(HOME)/.local

.PHONY: build run test install autostart release clean

# A single static executable, for any x86_64 Linux.
build:
	cargo build --release --target $(TARGET)
	@ls -l $(BIN)

run: build
	$(BIN)

test:
	cargo test

install: build
	install -Dm755 $(BIN) $(PREFIX)/bin/souris-facile-pingouin
	install -Dm644 resources/souris-facile-pingouin.desktop $(PREFIX)/share/applications/souris-facile-pingouin.desktop

# Starts it with every session.
autostart: install
	install -Dm644 resources/souris-facile-pingouin.desktop $(HOME)/.config/autostart/souris-facile-pingouin.desktop

# The executable and the desktop file, ready to share.
release: build
	rm -rf build && mkdir -p build/souris-facile-pingouin-$(VERSION)
	cp $(BIN) resources/souris-facile-pingouin.desktop README.md LICENSE build/souris-facile-pingouin-$(VERSION)/
	tar -C build -czf $(TARBALL) souris-facile-pingouin-$(VERSION)
	@echo $(TARBALL)

clean:
	cargo clean
	rm -rf build
