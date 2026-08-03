import { useUserStore } from '@/stores/user'
import { get, post } from './request'
import type { ReturnData } from '@/types'

/**
 * 服务端缓存本书 —— 后端契约（后端并行实现中，未就绪时 silent 降级提示）
 *
 * POST /reader3/cacheBookOnServer?url=<bookUrl>   → 启动后台整书缓存任务，立即返回
 *      ReturnData<{ started, url, cached, total, title }>
 * GET  /reader3/cacheBookSSE?url=<bookUrl>        → SSE 进度流（约每 300ms 一帧）
 *      data: { cached, total, title, finished, cancelled, error }
 * GET  /reader3/cancelCacheBook?url=<bookUrl>     → 取消任务（内存任务表移除）
 *
 * 说明：任务契约描述为「POST 即 SSE 进度」，后端实现为「POST 启动 + cacheBookSSE 轮询推送」，
 * 此处按后端实现对接（两者进度事件结构一致：{cached,total,title}）。
 * 传输层失败（网络错误 / 非 200）reject，由调用方 silent 降级提示。
 */

/** POST /reader3/cacheBookOnServer 启动结果 */
export interface CacheStartResult {
  started: boolean
  url?: string
  cached: number
  total: number
  title?: string
}

/** 缓存进度帧（SSE data） */
export interface CacheSSEProgress {
  cached: number
  total: number
  title?: string
  finished?: boolean
  cancelled?: boolean
  error?: string | null
}

export interface CacheProgressCallbacks {
  /** 进度帧到达 */
  onProgress: (p: CacheSSEProgress) => void
  /** 流正常结束（含 finished 帧后结束） */
  onEnd?: () => void
  /** 传输层失败（非用户关闭） */
  onStreamError: (msg: string) => void
}

export interface CacheProgressHandle {
  close: () => void
}

/** POST /reader3/cacheBookOnServer：启动后台缓存（silent——未就绪时调用方降级） */
export function cacheBookOnServer(url: string): Promise<ReturnData<CacheStartResult>> {
  return post<CacheStartResult>('/cacheBookOnServer', { url }, { silent: true })
}

interface ParsedSSEBlock {
  event: string
  data: string
}

function parseSSEBlock(block: string): ParsedSSEBlock | null {
  let event = ''
  const dataLines: string[] = []
  for (const rawLine of block.split('\n')) {
    const line = rawLine.endsWith('\r') ? rawLine.slice(0, -1) : rawLine
    if (line.startsWith('event:')) event = line.slice(6).trim()
    else if (line.startsWith('data:')) dataLines.push(line.slice(5).replace(/^ /, ''))
  }
  if (!dataLines.length) return null
  return { event, data: dataLines.join('\n') }
}

function tryJson(s: string): unknown {
  try {
    return JSON.parse(s) as unknown
  } catch {
    return null
  }
}

function dispatchSSEBlock(block: string, cbs: CacheProgressCallbacks) {
  const evt = parseSSEBlock(block)
  if (!evt || !evt.data) return
  const p = tryJson(evt.data) as (CacheSSEProgress & { type?: unknown; message?: unknown }) | null
  if (!p || typeof p !== 'object') return
  // 兼容两种命名：`event: progress` 与无名事件 + JSON.type=progress（后端当前为无名 data 帧）
  const isProgress =
    evt.event === 'progress' || evt.event === '' || evt.event === 'message' || p.type === 'progress'
  if (isProgress && typeof p.cached === 'number' && typeof p.total === 'number') {
    cbs.onProgress({
      cached: Math.max(0, p.cached),
      total: Math.max(0, p.total),
      title: typeof p.title === 'string' ? p.title : undefined,
      finished: p.finished === true,
      cancelled: p.cancelled === true,
      error: typeof p.error === 'string' ? p.error : null,
    })
    if (p.finished === true) cbs.onEnd?.()
  } else if (evt.event === 'error' || p.type === 'error') {
    cbs.onStreamError(typeof p.message === 'string' ? p.message : '缓存任务失败')
  }
}

async function consumeSSEStream(
  body: ReadableStream<Uint8Array>,
  cbs: CacheProgressCallbacks,
  closed: () => boolean,
): Promise<void> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      buffer = buffer.replace(/\r\n?/g, '\n')
      let sep: number
      while ((sep = buffer.indexOf('\n\n')) !== -1) {
        const block = buffer.slice(0, sep)
        buffer = buffer.slice(sep + 2)
        dispatchSSEBlock(block, cbs)
      }
    }
    if (buffer.trim()) dispatchSSEBlock(buffer, cbs)
    cbs.onEnd?.()
  } catch {
    if (closed()) return // 用户主动关闭
    cbs.onStreamError('缓存进度连接中断')
  }
}

/** GET /reader3/cacheBookSSE?url=：订阅缓存进度流（原生 fetch 流式读取，accessToken 手动附加） */
export function cacheBookSSE(url: string, cbs: CacheProgressCallbacks): Promise<CacheProgressHandle> {
  const controller = new AbortController()
  const token = useUserStore().accessToken
  const query = token
    ? `?url=${encodeURIComponent(url)}&accessToken=${encodeURIComponent(token)}`
    : `?url=${encodeURIComponent(url)}`
  return fetch(`/reader3/cacheBookSSE${query}`, {
    method: 'GET',
    headers: { Accept: 'text/event-stream' },
    signal: controller.signal,
  }).then(async (response) => {
    if (!response.ok) throw new Error(`缓存进度服务异常（HTTP ${response.status}）`)
    if (!response.body) throw new Error('缓存进度服务未返回数据流')
    void consumeSSEStream(response.body, cbs, () => controller.signal.aborted)
    return { close: () => controller.abort() } satisfies CacheProgressHandle
  })
}

/** GET /reader3/cancelCacheBook?url=：取消缓存任务（silent——未就绪时调用方降级） */
export function cancelCacheBook(url: string): Promise<ReturnData<{ cancelled: boolean }>> {
  return get<{ cancelled: boolean }>('/cancelCacheBook', { url }, { silent: true })
}
