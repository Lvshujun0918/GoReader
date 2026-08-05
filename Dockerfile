# ============================================================
# reader-dev (Rust) 多阶段构建
#   - 运行镜像内置 chromium：书源登录/滑块验证码/CF 质询浏览器流
#     （browser.rs CDP 自动发现 READER_CHROME_PATH=/usr/bin/chromium）
#   - GAP 175：运行镜像内置 python3 + camoufox（验证码求解 HTTP 后端
#     scripts/camoufox_solver.py——CDP 失败后的强质询兜底，端口 8196）
#   - 构建：docker build -t reader-dev .
# ============================================================

# ---------- 阶段 1：后端编译 ----------
FROM rust:1.97-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY .cargo ./.cargo
# GAP 176：epub 导出内嵌中文字体（include_bytes 编译期内嵌 web-ui/public/fonts/）
COPY web-ui/public/fonts ./web-ui/public/fonts
ENV RUSTFLAGS="--cfg reqwest_unstable"
RUN cargo build --release

# ---------- 阶段 2：前端构建 ----------
FROM node:20-slim AS web
WORKDIR /web
COPY web-ui/package.json web-ui/package-lock.json* ./
RUN npm install
COPY web-ui ./
RUN npm run build

# ---------- 阶段 2.5：camoufox 求解后端（pip 包 + 浏览器二进制，构建期下载） ----------
FROM python:3.12-slim AS camo
RUN pip install --no-cache-dir camoufox==0.5.4 \
    && python -m camoufox fetch

# ---------- 阶段 3：运行镜像 ----------
FROM debian:bookworm-slim

# chromium（验证码/CF 质询浏览器流）+ 时区 + CA + python3（camoufox 后端）
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium \
        ca-certificates \
        tzdata \
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

ENV TZ=Asia/Shanghai
ENV READER_APP_WEB_ROOT=/app/web-ui/dist
ENV READER_CHROME_PATH=/usr/bin/chromium
ENV READER_CAMOUFOX_URL=http://127.0.0.1:8196

COPY --from=builder /app/target/release/reader-dev /usr/local/bin/reader-dev
COPY --from=web /web/dist /app/web-ui/dist

EXPOSE 8080
VOLUME ["/data"]
CMD ["reader-dev"]
