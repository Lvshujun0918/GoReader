/**
 * SSE 事件流通用解析/分发（cacheBookSSE 等服务端整书缓存进度流共用）。
 * 纯函数 + 无外部依赖——node:test 可直接单测。
 */

export interface ParsedSSEEvent {
  event: string
  data: string
}

/** 解析一个 SSE 事件块（event: / data: 行，兼容 \r\n；多行 data 以 \n 拼接） */
export function parseSSEBlock(block: string): ParsedSSEEvent | null {
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

/** 通用块消费：按 \n\n 切分事件块并逐块回调（cacheBookSSE 等非标准事件流共用）。
 *  用户取消（isAborted）静默返回；连接中断回调 onStreamError（缺省文案「连接中断，请重试」）。 */
export async function consumeSSEStreamBlocks(
  body: ReadableStream<Uint8Array>,
  onBlock: (block: string) => void,
  isAborted: () => boolean,
  onStreamError?: (msg: string) => void,
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
        onBlock(block)
      }
    }
    if (buffer.trim()) onBlock(buffer)
  } catch {
    if (isAborted()) return // 用户主动取消
    onStreamError?.('连接中断，请重试')
  }
}
