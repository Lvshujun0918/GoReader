/**
 * 应用构建版本号（镜像 tag = git 短 SHA）。
 *
 * 来源：Dockerfile 前端构建阶段注入 VITE_APP_VERSION（CI 传 GIT_SHA）；
 * 本地开发（vite dev / 本地 build）无注入时为 "dev"。
 */
export const APP_VERSION: string = import.meta.env.VITE_APP_VERSION || 'dev'
