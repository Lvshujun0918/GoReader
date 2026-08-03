import { get, post } from './request'
import type { CacheClearType, CacheInfo, ContentSearchHit, ReturnData } from '@/types'

/**
 * 缓存管理 + 全书内容搜索 —— 后端契约
 *
 * ============================ 后端契约 ============================
 * GET  /reader3/getCacheInfo      → ReturnData<CacheInfo>
 *                                   （缓存统计：tocCount 目录缓存数 / chapterCount 章节缓存数 / totalBytes 总大小(字节)）
 * POST /reader3/clearCache        body: { type: 'toc' | 'chapter' | 'all' } → ReturnData<null>
 *                                   （清理目录缓存 / 章节缓存 / 全部）
 * GET  /reader3/searchBookContent → params { key, bookUrl } → ReturnData<ContentSearchHit[]>
 *                                   hit: { chapterIndex, title, snippet }
 *                                   （全书内容搜索，本地书正文逐章匹配）
 * ================================================================
 *
 * 说明：接口以 silent 模式调用（后端未实现/不可用时返回 404，静默失败由调用方降级展示，
 * 不弹全局错误提示）；后端实现后无需改调用方即可自动生效。
 */

/** GET /reader3/getCacheInfo（silent 探测；失败时调用方显示「后端待实现」） */
export function getCacheInfo(): Promise<ReturnData<CacheInfo>> {
  return get<CacheInfo>('/getCacheInfo', undefined, { silent: true })
}

/** POST /reader3/clearCache（body { type }；失败时调用方提示「后端待实现」） */
export function clearCache(type: CacheClearType): Promise<ReturnData<null>> {
  return post<null>('/clearCache', { type }, { silent: true })
}

/** GET /reader3/searchBookContent（params key + bookUrl → 章节命中列表；失败由调用方在搜索弹层内提示） */
export function searchBookContent(key: string, bookUrl: string): Promise<ReturnData<ContentSearchHit[]>> {
  return get<ContentSearchHit[]>('/searchBookContent', { key, bookUrl }, { silent: true })
}
