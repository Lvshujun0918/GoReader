import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseSSEBlock, consumeSSEStreamBlocks } from './sse.ts'

test('parseSSEBlock 解析 event:/data: 块（兼容 CRLF 与 data 前导空格）', () => {
  const evt = parseSSEBlock('event: book\ndata: {"lastIndex":0,"data":[]}\n\n')
  assert.equal(evt?.event, 'book')
  assert.equal(evt?.data, '{"lastIndex":0,"data":[]}')
  // CRLF 兼容
  const cr = parseSSEBlock('event: end\r\ndata: {"lastIndex":1,"isEnd":true}\r\n\r\n')
  assert.equal(cr?.event, 'end')
  assert.equal(cr?.data, '{"lastIndex":1,"isEnd":true}')
  // 无 data 行 → null
  assert.equal(parseSSEBlock('event: book\n\n'), null)
  // 多行 data 以换行拼接（SSE 语义）
  const multi = parseSSEBlock('event: end\ndata: {"a":1}\ndata: ,"b":2}\n\n')
  assert.equal(multi?.data, '{"a":1}\n,"b":2}')
})

test('consumeSSEStreamBlocks 按块回调 + 跨 chunk 拼接（cacheBookSSE 共用）', async () => {
  const blocks: string[] = []
  const raw =
    'data: {"type":"step","message":"1"}\n\ndata: {"type":"step","message":"2"}\n\ndata: {"type":"result","data":1}\n\n'
  const cut = Math.floor(raw.length / 2)
  const chunks = [raw.slice(0, cut), raw.slice(cut)]
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(new TextEncoder().encode(c))
      controller.close()
    },
  })
  await consumeSSEStreamBlocks(stream, (b) => blocks.push(b), () => false)
  assert.equal(blocks.length, 3)
  assert.match(blocks[0], /message.:.1/)
  assert.match(blocks[2], /result/)
})

test('consumeSSEStreamBlocks 连接中断回调 onStreamError；用户取消静默', async () => {
  let errMsg = ''
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode('data: {"a":1}\n\n'))
      controller.error(new Error('boom'))
    },
  })
  await consumeSSEStreamBlocks(
    stream,
    () => {},
    () => false,
    (msg) => {
      errMsg = msg
    },
  )
  assert.ok(errMsg.includes('连接中断'))

  // 用户取消：不回调 onStreamError
  errMsg = ''
  const stream2 = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.error(new Error('aborted'))
    },
  })
  await consumeSSEStreamBlocks(
    stream2,
    () => {},
    () => true,
    (msg) => {
      errMsg = msg
    },
  )
  assert.equal(errMsg, '')
})
