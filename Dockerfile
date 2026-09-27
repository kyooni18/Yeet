# syntax=docker/dockerfile:1

FROM node:22-bookworm AS node-toolchain

FROM rust:1-bookworm AS build
COPY --from=node-toolchain /usr/local/ /usr/local/
WORKDIR /src

COPY RuntimeSource/package.json RuntimeSource/package-lock.json ./RuntimeSource/
RUN npm --prefix RuntimeSource ci

COPY web/package.json web/package-lock.json ./web/
RUN npm --prefix web ci

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    npm --prefix RuntimeSource run build \
    && npm --prefix web run build \
    && cargo build --locked --release \
    && cp /src/target/release/yeet /tmp/yeet

FROM node:22-bookworm-slim AS runtime

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        openssh-client \
        procps \
        tini \
    && rm -rf /var/lib/apt/lists/*

ARG YEET_UID=1000
ARG YEET_GID=1000
RUN if [ "$(id -g node)" != "$YEET_GID" ]; then groupmod --non-unique --gid "$YEET_GID" node; fi \
    && if [ "$(id -u node)" != "$YEET_UID" ]; then usermod --non-unique --uid "$YEET_UID" node; fi \
    && mkdir -p /workspace /home/node/.config/yeet /usr/local/share/yeet/runtime \
    && chown -R node:node /workspace /home/node

COPY --from=build /tmp/yeet /usr/local/bin/yeet
COPY --from=build /src/RuntimeSource/dist /usr/local/share/yeet/runtime/dist
COPY --from=build /src/RuntimeSource/skills /usr/local/share/yeet/runtime/skills
COPY --from=build /src/RuntimeSource/package.json /usr/local/share/yeet/runtime/package.json

ENV HOME=/home/node \
    YEET_CONFIG_DIR=/home/node/.config/yeet \
    YEET_RUNTIME_DIR=/usr/local/share/yeet/runtime \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    TERM=xterm-256color

USER node:node
WORKDIR /workspace
EXPOSE 7332

ENTRYPOINT ["/usr/bin/tini", "-g", "--", "yeet"]
