# The allowance daemon. Build: docker build -t allowance .
FROM rust:1-slim-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl3 ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home allowance \
 && mkdir /data && chown 10001:10001 /data
COPY --from=build /src/target/release/allowance /usr/local/bin/allowance
USER 10001:10001
# Inside a container the daemon must listen beyond loopback, so it insists on
# ALLOWANCE_TOKEN (see the README).
ENV ALLOWANCE_LISTEN=0.0.0.0:4020 \
    ALLOWANCE_POLICY=/etc/allowance/policy.toml \
    ALLOWANCE_LEDGER=/data/ledger.jsonl
VOLUME /data
EXPOSE 4020
ENTRYPOINT ["allowance"]
CMD ["daemon"]
