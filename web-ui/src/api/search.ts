import { post } from './request'
import { useUserStore } from '@/stores/user'
import type { ReturnData, SearchBook } from '@/types'

/** POST /reader3/searchBookMulti：多书源并发搜索（body {key, maxSources}；signal 可中止请求） */
export function searchBookMulti(key: string, maxSources = 50, signal?: AbortSignal): Promise<ReturnData<SearchBook[]>> {
  return post<SearchBook[]>('/searchBookMulti', { key, maxSources }, { signal })
}

/* ================= SSE 流式搜索（/reader3/searchBookMultiSSE） ================= */

export interface SearchSSEParams {
  key: string
  /** 书源分组过滤（空串 = 全部） */
  bookSourceGroup?: string
  /** 起始索引（-1 = 从头搜索） */
  lastIndex?: number
  /** 本次搜索覆盖的书源数量 */
  searchSize?: number
  /** 并发数 */
  concurrentCount?: number
}

export interface SearchSSECallbacks {
  /** 单个书源结果到达（data 可能为空数组） */
  onBooks: (lastIndex: number, books: SearchBook[]) => void
  /** 流正常结束（event: end） */
  onEnd: (lastIndex: number, isEnd: boolean) => void
  /** 服务端业务错误（event: error，data 为 ReturnData） */
  onErrorEvent: (ret: ReturnData) => void
  /** 流中途中断（连接断开，非用户取消） */
  onStreamError?: (msg: string) => void
}

export interface SearchSSEHandle {
  abort: () => void
}

interface ParsedSSEEvent {
  event: string
  data: string
}

/** 解析一个 SSE 事件块（event: / data: 行，兼容 \r\n） */
function parseSSEBlock(block: string): ParsedSSEEvent | null {
  let event = ''
  const dataLines: string[] = []
  for (const rawLine of block.split('\n')) {
    const line = rawLine.endsWith('\r') ? rawLine.slice(0, -1) : rawLine
    if (line.startsWith('event:')) {
      event = line.slice(6).trim()
    } else if (line.startsWith('data:')) {
      dataLines.push(line.slice(5).replace(/^ /, ''))
    }
  }
  if (!dataLines.length) return null
  return { event, data: dataLines.join('\n') }
}

/** 分发一个 SSE 事件块到对应回调 */
function dispatchSSEBlock(block: string, cbs: SearchSSECallbacks) {
  const evt = parseSSEBlock(block)
  if (!evt || !evt.data) return
  if (evt.event === 'book') {
    try {
      const payload = JSON.parse(evt.data) as { lastIndex?: number; data?: SearchBook[] }
      cbs.onBooks(payload.lastIndex ?? -1, Array.isArray(payload.data) ? payload.data : [])
    } catch {
      // 忽略无法解析的数据块
    }
  } else if (evt.event === 'end') {
    try {
      const payload = JSON.parse(evt.data) as { lastIndex?: number; isEnd?: boolean }
      cbs.onEnd(payload.lastIndex ?? -1, payload.isEnd ?? false)
    } catch {
      cbs.onEnd(-1, false)
    }
  } else if (evt.event === 'error') {
    try {
      cbs.onErrorEvent(JSON.parse(evt.data) as ReturnData)
    } catch {
      cbs.onErrorEvent({ isSuccess: false, errorMsg: evt.data, data: null })
    }
  }
}

/** 增量消费 ReadableStream，按 \n\n 切分事件块 */
async function consumeSSEStream(
  body: ReadableStream<Uint8Array>,
  cbs: SearchSSECallbacks,
  isAborted: () => boolean,
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
  } catch (err) {
    if (isAborted()) return // 用户主动取消
    cbs.onStreamError?.('连接中断，请重试')
  }
}

/**
 * POST /reader3/searchBookMultiSSE：多书源流式搜索（原生 fetch，不走 axios）
 * - accessToken 手动附加 query（SSE 无 axios 拦截器）
 * - 传输层失败（网络错误 / 非 200 / 非 event-stream 响应）reject，调用方可降级 searchBookMulti
 */
export function searchBookMultiSSE(
  params: SearchSSEParams,
  cbs: SearchSSECallbacks,
): Promise<SearchSSEHandle> {
  const controller = new AbortController()
  const token = useUserStore().accessToken
  const query = token ? `?accessToken=${encodeURIComponent(token)}` : ''
  const body: Record<string, unknown> = { key: params.key }
  if (params.bookSourceGroup !== undefined) body.bookSourceGroup = params.bookSourceGroup
  if (params.lastIndex !== undefined) body.lastIndex = params.lastIndex
  if (params.searchSize !== undefined) body.searchSize = params.searchSize
  if (params.concurrentCount !== undefined) body.concurrentCount = params.concurrentCount

  return fetch(`/reader3/searchBookMultiSSE${query}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'text/event-stream' },
    body: JSON.stringify(body),
    signal: controller.signal,
  }).then(async (response) => {
    if (!response.ok) throw new Error(`搜索服务异常（HTTP ${response.status}）`)
    const contentType = response.headers.get('content-type') ?? ''
    if (contentType && !contentType.includes('text/event-stream')) {
      throw new Error('当前服务不支持流式搜索')
    }
    if (!response.body) throw new Error('当前服务不支持流式搜索')
    let aborted = false
    void consumeSSEStream(response.body, cbs, () => aborted)
    return {
      abort: () => {
        aborted = true
        controller.abort()
      },
    } satisfies SearchSSEHandle
  })
}
