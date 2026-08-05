/**
 * 夜读 Reader · Service Worker（PWA 离线壳）
 *
 * 缓存策略：
 * - 核心壳（导航请求 / 页面 HTML：index.html + JS/CSS 等构建产物）→ 网络优先 + 缓存回退
 *   （保证每次访问尽量拿到最新版本；离线时回退缓存壳，SPA 前端路由照常工作）
 * - 静态资源（图片 / 字体 / manifest 等）→ 缓存优先（hash 文件名天然免疫陈旧内容）
 * - 跨域请求（书源封面 / 正文等）一律不缓存、不拦截
 *
 * 版本号：发版时递增 CACHE_VERSION 即可让旧缓存整体失效（activate 清理）。
 */
const CACHE_VERSION = 'reader-shell-v1'
const SHELL_CACHE = `${CACHE_VERSION}-shell`
const STATIC_CACHE = `${CACHE_VERSION}-static`

/** 预缓存的离线壳（install 时写入，供离线首屏） */
const PRECACHE_URLS = ['/', '/index.html', '/manifest.webmanifest']

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then((cache) => cache.addAll(PRECACHE_URLS))
      .then(() => self.skipWaiting()),
  )
})

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((k) => k !== SHELL_CACHE && k !== STATIC_CACHE)
            .map((k) => caches.delete(k)),
        ),
      )
      .then(() => self.clients.claim()),
  )
})

/** 是否同源 GET 请求 */
function isSameOriginGet(request) {
  if (request.method !== 'GET') return false
  const url = new URL(request.url)
  return url.origin === self.location.origin
}

/** 网络优先 + 缓存回退（核心壳：导航 HTML） */
async function networkFirst(request) {
  const cache = await caches.open(SHELL_CACHE)
  try {
    const response = await fetch(request)
    if (response && response.ok) {
      // SPA：所有导航响应都以 index.html 为缓存键，避免每个路由路径各存一份
      await cache.put('/index.html', response.clone())
    }
    return response
  } catch {
    const cached = await cache.match('/index.html')
    if (cached) return cached
    // 极端情况（首次访问即离线）：连壳都没有，交给浏览器报错
    throw new Error('offline: no cached shell')
  }
}

/** 缓存优先 + 网络回填（静态资源：hash 文件名 / 字体 / 图片 / manifest） */
async function cacheFirst(request) {
  const cache = await caches.open(STATIC_CACHE)
  const cached = await cache.match(request)
  if (cached) return cached
  try {
    const response = await fetch(request)
    if (response && response.ok) {
      await cache.put(request, response.clone())
    }
    return response
  } catch {
    if (cached) return cached
    throw new Error(`offline: ${request.url}`)
  }
}

self.addEventListener('fetch', (event) => {
  const { request } = event
  if (!isSameOriginGet(request)) return

  // 导航请求（页面 / 前端路由）→ 网络优先 + 缓存回退
  if (request.mode === 'navigate') {
    event.respondWith(networkFirst(request).catch(() => caches.match('/index.html')))
    return
  }

  // 静态资源 → 缓存优先（构建产物带 hash，安全）；失败不拿 HTML 冒充
  event.respondWith(cacheFirst(request))
})
