import { get, post } from './request'
import type { CacheInfo, ContentSearchHit, ReturnData } from '@/types'

/**
 * 缓存管理 + 全书内容搜索 —— 后端契约（当前后端未实现，均为「待实现」标注）
 *
 * ============================ 后端契约 ============================
 * GET  /reader3/getCacheInfo      → ReturnData<CacheInfo>
 *                                   （缓存统计：chapterCacheCount 章节缓存数 / chapterCacheSize 章节缓存大小(字节)）
 * POST /reader3/clearCache        → ReturnData<null>（清理章节缓存）
 * GET  /reader3/searchBookContent → params { key, bookUrl } → ReturnData<ContentSearchHit[]>
 *                                   hit: { chapterIndex, title, snippet }
 *                                   （全书内容搜索：正文可逐章调 getBookContent 或由后端索引；
 *                                     后端未就绪时前端降级——本地搜索已加载章节，或标注待实现）
 * ================================================================
 *
 * 说明：三个接口均以 silent 模式调用（后端未实现时返回 404，静默失败由调用方降级展示，
 * 不弹全局错误提示）；后端实现后无需改调用方即可自动生效。
 */

/** GET /reader3/getCacheInfo（待实现；失败时调用方显示「后端待实现」） */
export function getCacheInfo(): Promise<ReturnData<CacheInfo>> {
  return get<CacheInfo>('/getCacheInfo', undefined, { silent: true })
}

/** POST /reader3/clearCache（待实现；失败时调用方提示「后端待实现」） */
export function clearCache(): Promise<ReturnData<null>> {
  return post<null>('/clearCache', undefined, { silent: true })
}

/** GET /reader3/searchBookContent（待实现；params key + bookUrl → 章节命中列表） */
export function searchBookContent(key: string, bookUrl: string): Promise<ReturnData<ContentSearchHit[]>> {
  return get<ContentSearchHit[]>('/searchBookContent', { key, bookUrl }, { silent: true })
}
