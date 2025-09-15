#!/bin/zsh

cargo clean
# TODO: we're seeing some kind of race condition with the proc macros that 2 successive builds gets around
cargo build --release || cargo build --release

sudo cp target/release/vizier /usr/local/bin/
