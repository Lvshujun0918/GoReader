# ============================================================
# reader-dev (Rust) 多阶段构建
#   - 运行镜像内置 chromium：书源登录/滑块验证码/CF 质询浏览器流
#     （browser.rs CDP 自动发现 READER_CHROME_PATH=/usr/bin/chromium）
#   - 构建：docker build -t reader-dev .
# ============================================================

# ---------- 阶段 1：后端编译 ----------
FROM rust:1.85-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ---------- 阶段 2：前端构建 ----------
FROM node:20-slim AS web
WORKDIR /web
COPY web-ui/package.json web-ui/package-lock.json* ./
RUN npm install
COPY web-ui ./
RUN npm run build

# ---------- 阶段 3：运行镜像 ----------
FROM debian:bookworm-slim

# chromium（验证码/CF 质询浏览器流）+ 时区 + CA
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium \
        ca-certificates \
        tzdata \
        fonts-noto-cjk \
    && rm -rf /var/lib/apt/lists/*

ENV TZ=Asia/Shanghai
ENV READER_APP_WEB_ROOT=/app/web-ui/dist
ENV READER_CHROME_PATH=/usr/bin/chromium

COPY --from=builder /app/target/release/reader-dev /usr/local/bin/reader-dev
COPY --from=web /web/dist /app/web-ui/dist

EXPOSE 8080
VOLUME ["/data"]
CMD ["reader-dev"]
