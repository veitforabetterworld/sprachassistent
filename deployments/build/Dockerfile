# Cross-compile environment for the Raspberry Pi 5 (aarch64). Debian bookworm on both sides, so the
# binary only needs the glibc the Pi already has. Used by GitHub Actions and locally:
#   docker build -t sprachassistent-build deployments/build
#   docker run --rm -v "$PWD:/src" sprachassistent-build
FROM debian:bookworm

RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      ca-certificates curl bzip2 git pkg-config build-essential cmake clang libclang-dev \
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross binutils-aarch64-linux-gnu \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
      --target aarch64-unknown-linux-gnu

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_gnu="--sysroot=/usr/aarch64-linux-gnu" \
    # Pi 5 CPU: enables the dotprod/fp16 kernels of whisper.cpp (about a third faster than generic
    # armv8). The binary then no longer runs on a Pi 4 (Cortex-A72).
    CFLAGS_aarch64_unknown_linux_gnu="-mcpu=cortex-a76" \
    CXXFLAGS_aarch64_unknown_linux_gnu="-mcpu=cortex-a76"

WORKDIR /src
CMD ["bash", "deployments/build/build-pi.sh"]
