import type { ReplaceRule, ReturnData } from '@/types'

/**
 * 替换规则存储层 —— 当前为 localStorage 占位实现，后端就绪后切换为真实请求。
 *
 * ============================ 后端契约（约定，待后端实现） ============================
 * GET  /reader3/getReplaceRules    → ReturnData<ReplaceRule[]>
 * POST /reader3/saveReplaceRule    body: ReplaceRule        → ReturnData<null>
 * POST /reader3/saveReplaceRules   body: ReplaceRule[]      → ReturnData<{ count: number }>
 * POST /reader3/deleteReplaceRule  body: { id: string }     → ReturnData<null>
 * ==============================================================================
 * localStorage key: reader_replace_rules（值为 ReplaceRule[] 的 JSON）
 */

const STORAGE_KEY = 'reader_replace_rules'

/** 同步读取（阅读页渲染时直接使用；localStorage 异常时返回空数组） */
export function loadReplaceRules(): ReplaceRule[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw) as unknown
    if (!Array.isArray(arr)) return []
    return (arr as ReplaceRule[]).filter((r) => r && typeof r === 'object' && typeof r.find === 'string')
  } catch {
    return []
  }
}

/** 同步持久化整表 */
export function persistReplaceRules(rules: ReplaceRule[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(rules))
  } catch {
    /* localStorage 满/不可用：忽略 */
  }
}

/** GET /reader3/getReplaceRules（占位：读 localStorage） */
export function getReplaceRules(): Promise<ReturnData<ReplaceRule[]>> {
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: loadReplaceRules() })
}

/** POST /reader3/saveReplaceRule（占位：写 localStorage，id 相同则覆盖） */
export function saveReplaceRule(rule: ReplaceRule): Promise<ReturnData<null>> {
  const list = loadReplaceRules()
  const i = list.findIndex((r) => r.id === rule.id)
  if (i >= 0) list[i] = rule
  else list.push(rule)
  persistReplaceRules(list)
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: null })
}

/** POST /reader3/deleteReplaceRule（占位：按 id 删除 localStorage 记录） */
export function deleteReplaceRule(id: string): Promise<ReturnData<null>> {
  persistReplaceRules(loadReplaceRules().filter((r) => r.id !== id))
  return Promise.resolve({ isSuccess: true, errorMsg: '', data: null })
}
