.PHONY: build install run clean

build:
	cargo build --release

install: build
	mkdir -p ~/.local/bin
	install -m 755 ../target/release/cce-data-editor ~/.local/bin/cce-data-editor

run:
	cargo run

clean:
	cargo clean
