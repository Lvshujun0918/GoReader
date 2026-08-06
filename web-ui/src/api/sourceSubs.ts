import { get, post } from './request'
import { onBackendReachable } from './backendFlag'
import type { ReturnData, SourceSub } from '@/types'

/**
 * 书源订阅存储层 —— 后端为主（/reader3/getSourceSubs 等），localStorage 为降级缓存：
 * - 后端可用：读写走服务端（账号内多设备一致）
 * - 后端失败：降级 localStorage（reader_source_subs），功能不中断
 *
 * ============================ 后端契约 ============================
 * GET  /reader3/getSourceSubs      → ReturnData<SourceSub[]>
 * POST /reader3/saveSourceSub      body: { url, name } → ReturnData<{ count }>
 *                                   （服务端抓取校验远程书源 JSON → 订阅入库 + 批量导入书源表）
 * POST /reader3/refreshSourceSub   body: { url }       → ReturnData<{ count }>
 *                                   （重新拉取并覆盖导入书源；订阅需已存在）
 * POST /reader3/deleteSourceSub    body: { url }       → ReturnData<null>
 *                                   （仅删订阅行，不影响已导入书源）
 * ================================================================
 * localStorage key: reader_source_subs（值为 SourceSub[] 的 JSON）
 * 订阅只记录远程书源地址与名称；书源数据由后端 saveSourceSub/refreshSourceSub 导入，
 * 降级模式下由调用方前端 fetch + saveBookSources 导入。
 */

const STORAGE_KEY = 'reader_source_subs'

/** 同步读取（localStorage 异常时返回空数组） */
export function loadSourceSubs(): SourceSub[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw) as unknown
    if (!Array.isArray(arr)) return []
    return (arr as SourceSub[]).filter((s) => s && typeof s === 'object' && typeof s.url === 'string')
  } catch {
    return []
  }
}

/** 同步持久化整表 */
export function persistSourceSubs(subs: SourceSub[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(subs))
  } catch {
    /* localStorage 满/不可用：忽略 */
  }
}

/** 后端不可达标志（本模块内短路，避免每次操作都等超时） */
let backendDown = false

/** GET /reader3/getSourceSubs（后端优先；失败降级 localStorage 并镜像缓存） */
export async function getSourceSubs(): Promise<ReturnData<SourceSub[]>> {
  if (backendDown) {
    return { isSuccess: false, errorMsg: '服务端暂不可用，已降级本地数据', data: loadSourceSubs() }
  }
  try {
    const res = await get<SourceSub[]>('/getSourceSubs', undefined, { silent: true })
    persistSourceSubs(res.data ?? [])
    return res
  } catch {
    backendDown = true
    return { isSuccess: false, errorMsg: '服务端暂不可用，已降级本地数据', data: loadSourceSubs() }
  }
}

/**
 * POST /reader3/saveSourceSub（后端优先：抓取校验 + 订阅入库 + 批量导入书源，返回导入数）。
 * 降级（后端不可达）：仅写入 localStorage，data=null —— 调用方需自行导入书源（fetch + saveBookSources）。
 */
export async function saveSourceSub(
  url: string,
  name: string,
): Promise<ReturnData<{ count: number } | null>> {
  if (!backendDown) {
    try {
      const res = await post<{ count: number }>('/saveSourceSub', { url, name }, { silent: true })
      const list = loadSourceSubs()
      const existing = list.find((s) => s.url === url)
      if (existing) {
        existing.name = name
      } else {
        list.push({ url, name, enabled: true })
      }
      persistSourceSubs(list)
      return res
    } catch {
      backendDown = true
    }
  }
  const list = loadSourceSubs()
  const existing = list.find((s) => s.url === url)
  if (existing) {
    existing.name = name
  } else {
    list.push({ url, name, enabled: true })
  }
  persistSourceSubs(list)
  return { isSuccess: false, errorMsg: '服务端暂不可用，已降级本地数据', data: null }
}

/** POST /reader3/deleteSourceSub（后端优先；失败降级 localStorage） */
export async function deleteSourceSub(url: string): Promise<ReturnData<null>> {
  if (!backendDown) {
    try {
      const res = await post<null>('/deleteSourceSub', { url }, { silent: true })
      persistSourceSubs(loadSourceSubs().filter((s) => s.url !== url))
      return res
    } catch {
      backendDown = true
    }
  }
  persistSourceSubs(loadSourceSubs().filter((s) => s.url !== url))
  return { isSuccess: false, errorMsg: '服务端暂不可用，已降级本地数据', data: null }
}

/**
 * POST /reader3/refreshSourceSub（后端优先：重新拉取远程书源 JSON 并覆盖导入书源表，返回导入数；
 * 订阅不存在返回业务失败）。失败返回 isSuccess=false（不抛异常），由调用方降级为前端 fetch + saveBookSources 导入。
 */
export async function refreshSourceSub(url: string): Promise<ReturnData<{ count: number }>> {
  if (backendDown) {
    return { isSuccess: false, errorMsg: '', data: { count: 0 } }
  }
  try {
    return await post<{ count: number }>('/refreshSourceSub', { url }, { silent: true })
  } catch {
    backendDown = true
    return { isSuccess: false, errorMsg: '', data: { count: 0 } }
  }
}

/** 恢复后端调用（登录态变化/网络恢复时由上层调用） */
export function resetBackendFlag(): void {
  backendDown = false
}

// P2：任一后端请求成功（request.ts 拦截器）即复位短路标志——网络恢复后自动回到后端优先
onBackendReachable(resetBackendFlag)
