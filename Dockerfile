# ============================================================
# reader-dev (Go) 多阶段构建
#   - Go 后端：CGO_ENABLED=0 静态链接（纯 Go SQLite 驱动 glebarez/sqlite，
#     无 C 依赖；goja JS 引擎纯 Go——比 Rust 版 musl 构建更简单）
#   - 前端：web-ui（Vue 3 + shadcn-vue）→ dist
#   - 运行镜像内置 obscura（浏览器后端）+ python3/camoufox（验证码求解）
#   - 构建：docker build -t reader-dev .
# ============================================================

# ---------- 阶段 1：Go 后端编译 ----------
FROM golang:1.25 AS builder
WORKDIR /app
# 依赖层缓存（go.mod/go.sum 未变则复用层）
COPY go.mod go.sum ./
RUN go mod download
COPY cmd ./cmd
COPY internal ./internal
ENV CGO_ENABLED=0
RUN go build -trimpath -ldflags="-s -w" -o /out/reader-dev ./cmd/server

# ---------- 阶段 2：前端构建 ----------
FROM node:20-slim AS web
WORKDIR /web
COPY web-ui/package.json web-ui/package-lock.json* ./
RUN npm install
COPY web-ui ./
RUN npm run build

# ---------- 阶段 3：camoufox 求解后端（pip 包 + 浏览器二进制，构建期下载） ----------
FROM python:3.12-slim AS camo
RUN pip install --no-cache-dir camoufox==0.5.4 \
    && python -m camoufox fetch

# ---------- 阶段 4：obscura 浏览器（release stealth 构建——BoringSSL TLS 指纹模拟/反检测/追踪器拦截） ----------
FROM debian:trixie-slim AS obscura
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -eux; \
    case "${TARGETARCH:-}" in \
      ""|amd64|x86_64) ASSET="obscura-x86_64-linux-stealth.tar.gz" ;; \
      arm64|aarch64) ASSET="obscura-aarch64-linux-stealth.tar.gz" ;; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fL --retry 3 -o /tmp/obscura.tar.gz \
      "https://github.com/h4ckf0r0day/obscura/releases/latest/download/${ASSET}"; \
    mkdir -p /opt/obscura; \
    tar xzf /tmp/obscura.tar.gz -C /opt/obscura; \
    rm /tmp/obscura.tar.gz; \
    test -x /opt/obscura/obscura; \
    ls -la /opt/obscura

# ---------- 阶段 5：运行镜像 ----------
FROM debian:trixie-slim

# 时区 + CA + tini（PID 1 信号转发）+ python3（camoufox 后端）
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        tzdata \
        tini \
        fonts-noto-cjk \
        python3 \
        python3-pip \
    && rm -rf /var/lib/apt/lists/*

# camoufox 运行时系统库（Firefox 内核——playwright firefox 依赖集）
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libnss3 libnspr4 libdbus-1-3 libatk1.0-0 libatk-bridge2.0-0 \
        libcups2 libdrm2 libxkbcommon0 libxcomposite1 libxdamage1 \
        libxfixes3 libxrandr2 libgbm1 libasound2 libpango-1.0-0 libcairo2 \
    && rm -rf /var/lib/apt/lists/*

# camoufox：pip 包 + 浏览器二进制（从 camo 阶段拷贝——免容器内在线下载）
COPY --from=camo /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages
COPY --from=camo /root/.cache/camoufox /root/.cache/camoufox
COPY scripts/camoufox_solver.py /usr/local/bin/camoufox_solver.py

# obscura 浏览器（唯一后端——stealth 构建：BoringSSL TLS 指纹模拟/反检测/追踪器拦截）
COPY --from=obscura /opt/obscura /opt/obscura

ENV TZ=Asia/Shanghai
ENV READER_APP_WEB_ROOT=/app/web-ui/dist
ENV READER_OBSCURA_BIN=/opt/obscura/obscura
ENV READER_CAMOUFOX_URL=http://127.0.0.1:8196

COPY --from=builder /out/reader-dev /usr/local/bin/reader-dev
COPY --from=web /web/dist /app/web-ui/dist

EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["reader-dev"]
