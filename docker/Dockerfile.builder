# synthpass-builder — the local dev/verify-loop image for machines that lack a
# native cmake/clang toolchain (see docs' build-environment notes). Mirrors
# `.github/workflows/ci.yml`'s `rust` job's apt list (source of truth for
# system deps) plus the nightly toolchain `fuzz` needs and, as of v1.0.0, the
# musl target + pinned Zig + cargo-zigbuild used to cross-compile the static
# release binaries (see docs/ARCHITECTURE.md §10).
#
# This replaces the old ad-hoc `docker commit`-built `synthpass-builder:latest` —
# build it explicitly instead:
#   docker build -f docker/Dockerfile.builder -t synthpass-builder:latest .
#
# Usage (from repo root, Git Bash on Windows needs MSYS_NO_PATHCONV=1 to stop
# `-w /work` from being mangled into a Windows path):
#   MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" \
#     -v synthpass_target:/work/target -v synthpass_cargo_registry:/usr/local/cargo/registry \
#     -w /work synthpass-builder:latest bash -c "cargo test -p mrz"
#
# Cross-compiling to musl locally:
#   MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" \
#     -v synthpass_target:/work/target -v synthpass_cargo_registry:/usr/local/cargo/registry \
#     -w /work synthpass-builder:latest \
#     cargo zigbuild --release --target x86_64-unknown-linux-musl -p synthpass-cli -p synthpass-serve

FROM rust:1.96

ARG ZIG_VERSION=0.13.0

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
    cmake pkg-config clang libclang-dev \
    curl xz-utils ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Pinned Zig release — provides a versioned clang + musl sysroot, used as the
# CC/CXX for cargo-zigbuild's musl cross-compiles (see docs/ARCHITECTURE.md
# §10 for why Zig was chosen over cross-rs / manual musl-gcc).
RUN curl -fL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-x86_64-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
 && tar -xJf /tmp/zig.tar.xz -C /opt \
 && ln -s "/opt/zig-linux-x86_64-${ZIG_VERSION}/zig" /usr/local/bin/zig \
 && rm /tmp/zig.tar.xz

RUN rustup target add x86_64-unknown-linux-musl \
 && rustup toolchain install nightly \
 && rustup component add rustfmt clippy \
 && cargo install cargo-zigbuild --locked \
 && cargo install cargo-fuzz --locked
