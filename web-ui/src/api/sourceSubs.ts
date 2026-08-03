import type { ReturnData, SourceSub } from '@/types'

/**
 * 书源订阅存储层 —— 当前为 localStorage 占位实现，后端就绪后切换为真实请求。
 *
 * ============================ 后端契约（约定，待后端实现） ============================
 * GET  /reader3/getSourceSubs     → ReturnData<SourceSub[]>
 * POST /reader3/saveSourceSubs    body: SourceSub[]      → ReturnData<{ count: number }>
 * ==============================================================================
 * localStorage key: reader_source_subs（值为 SourceSub[] 的 JSON）
 * 订阅本身只记录远程书源地址；书源数据始终通过 saveBookSources 批量导入到书源表。
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

/** GET /reader3/getSourceSubs（占位：读 localStorage） */
export function getSourceSubs(): Promise<ReturnData<SourceSub[]>> {
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: loadSourceSubs() })
}

/** POST /reader3/saveSourceSubs（占位：写 localStorage 整表） */
export function saveSourceSubs(subs: SourceSub[]): Promise<ReturnData<{ count: number }>> {
  persistSourceSubs(subs)
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: { count: subs.length } })
}
