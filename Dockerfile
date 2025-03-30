FROM ubuntu AS build
ENV HOME="/root"
WORKDIR $HOME

RUN apt update \
  && apt install -y --no-install-recommends \
  build-essential \
  curl \
  python3-venv \
  cmake \
  && apt clean \
  && rm -rf /var/lib/apt/lists/*

# Setup zig as cross compiling linker
RUN python3 -m venv $HOME/.venv
RUN .venv/bin/pip install cargo-zigbuild
ENV PATH="$HOME/.venv/bin:$PATH"

# Install rust
RUN echo "aarch64-unknown-linux-musl" > rust_target.txt
COPY rust-toolchain.toml rust-toolchain.toml
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --target $(cat rust_target.txt) --profile minimal --default-toolchain none
ENV PATH="$HOME/.cargo/bin:$PATH"
# Install the toolchain then the musl target
RUN rustup toolchain install
RUN rustup target add $(cat rust_target.txt)

# Build
COPY src src
COPY ./Cargo.toml Cargo.toml
COPY ./Cargo.lock Cargo.lock
RUN cargo zigbuild --target $(cat rust_target.txt) --release
RUN cp target/$(cat rust_target.txt)/release/gitblog /gitblog


FROM scratch
COPY --from=build /gitblog /gitblog
CMD ["/gitblog"]
