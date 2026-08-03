import { get, post } from './request'
import type { HttpTts, ReturnData } from '@/types'

/**
 * HttpTTS 听书源存储层 —— 后端为主（/reader3/getHttpTTSList 等），localStorage 为降级缓存：
 * - 后端可用：读写走服务端（账号内多设备一致）
 * - 后端失败：降级 localStorage，功能不中断
 *
 * ============================ 后端契约 ============================
 * GET  /reader3/getHttpTTSList    → ReturnData<HttpTts[]>
 * POST /reader3/saveHttpTTS       body: HttpTts         → ReturnData<null>
 * POST /reader3/deleteHttpTTS     body: { id: string }  → ReturnData<null>
 * ================================================================
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

/** 后端不可达标志（本模块内短路，避免每次操作都等超时） */
let backendDown = false

/** GET /reader3/getHttpTTSList（后端优先；失败降级 localStorage 并镜像缓存） */
export async function getHttpTtsList(): Promise<ReturnData<HttpTts[]>> {
  if (backendDown) {
    return { isSuccess: true, errorMsg: '', data: loadHttpTtsList() }
  }
  try {
    const res = await get<HttpTts[]>('/getHttpTTSList')
    persistHttpTtsList(res.data ?? [])
    return res
  } catch {
    backendDown = true
    return { isSuccess: true, errorMsg: '', data: loadHttpTtsList() }
  }
}

/** POST /reader3/saveHttpTTS（后端优先；失败降级 localStorage，id 相同则覆盖） */
export async function saveHttpTts(tts: HttpTts): Promise<ReturnData<null>> {
  if (!backendDown) {
    try {
      const res = await post<null>('/saveHttpTTS', tts)
      const list = loadHttpTtsList()
      const i = list.findIndex((t) => t.id === tts.id)
      if (i >= 0) list[i] = tts
      else list.push(tts)
      persistHttpTtsList(list)
      return res
    } catch {
      backendDown = true
    }
  }
  const list = loadHttpTtsList()
  const i = list.findIndex((t) => t.id === tts.id)
  if (i >= 0) list[i] = tts
  else list.push(tts)
  persistHttpTtsList(list)
  return { isSuccess: true, errorMsg: '', data: null }
}

/** POST /reader3/deleteHttpTTS（后端优先；失败降级 localStorage） */
export async function deleteHttpTts(id: string): Promise<ReturnData<null>> {
  if (!backendDown) {
    try {
      const res = await post<null>('/deleteHttpTTS', { id })
      persistHttpTtsList(loadHttpTtsList().filter((t) => t.id !== id))
      return res
    } catch {
      backendDown = true
    }
  }
  persistHttpTtsList(loadHttpTtsList().filter((t) => t.id !== id))
  return { isSuccess: true, errorMsg: '', data: null }
}

/** 恢复后端调用（登录态变化/网络恢复时由上层调用） */
export function resetBackendFlag(): void {
  backendDown = false
}
