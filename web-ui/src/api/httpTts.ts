import type { HttpTts, ReturnData } from '@/types'

/**
 * HttpTTS 听书源存储层 —— 当前为 localStorage 占位实现，后端就绪后切换为真实请求。
 *
 * ============================ 后端契约（约定，待后端实现） ============================
 * GET  /reader3/getHttpTTSList    → ReturnData<HttpTts[]>
 * POST /reader3/saveHttpTTS       body: HttpTts         → ReturnData<null>
 * POST /reader3/deleteHttpTTS     body: { id: string }  → ReturnData<null>
 * ==============================================================================
 * localStorage key: reader_http_tts_list（值为 HttpTts[] 的 JSON）
 * type 参考 legado HttpTTS：0=在线合成（http 请求音频），1=本地引擎（预留）
 */

const STORAGE_KEY = 'reader_http_tts_list'

/** 同步读取（localStorage 异常时返回空数组） */
export function loadHttpTtsList(): HttpTts[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw) as unknown
    if (!Array.isArray(arr)) return []
    return (arr as HttpTts[]).filter((t) => t && typeof t === 'object' && typeof t.url === 'string')
  } catch {
    return []
  }
}

function persistHttpTtsList(list: HttpTts[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(list))
  } catch {
    /* localStorage 满/不可用：忽略 */
  }
}

/** GET /reader3/getHttpTTSList（占位：读 localStorage） */
export function getHttpTtsList(): Promise<ReturnData<HttpTts[]>> {
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: loadHttpTtsList() })
}

/** POST /reader3/saveHttpTTS（占位：写 localStorage，id 相同则覆盖） */
export function saveHttpTts(tts: HttpTts): Promise<ReturnData<null>> {
  const list = loadHttpTtsList()
  const i = list.findIndex((t) => t.id === tts.id)
  if (i >= 0) list[i] = tts
  else list.push(tts)
  persistHttpTtsList(list)
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: null })
}

/** POST /reader3/deleteHttpTTS（占位：按 id 删除 localStorage 记录） */
export function deleteHttpTts(id: string): Promise<ReturnData<null>> {
  persistHttpTtsList(loadHttpTtsList().filter((t) => t.id !== id))
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: null })
}
