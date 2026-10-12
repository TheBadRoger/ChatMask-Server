FROM rust:1.94-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN groupadd --gid 10001 chatmask && useradd --uid 10001 --gid chatmask --no-create-home chatmask && mkdir /data && chown chatmask:chatmask /data
COPY --from=build /build/target/release/chatmask-server /usr/local/bin/chatmask-server
USER chatmask
EXPOSE 7001/udp
STOPSIGNAL SIGTERM
ENTRYPOINT ["chatmask-server"]
CMD ["--key", "/data/server.key"]
