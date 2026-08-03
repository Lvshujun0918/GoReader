import { get, post } from './request'
import type { BookSource, ReturnData } from '@/types'

/** GET /reader3/getBookSources：当前用户书源列表 */
export function getBookSources(): Promise<ReturnData<BookSource[]>> {
  return get<BookSource[]>('/getBookSources')
}

/** POST /reader3/saveBookSource：保存单个书源（body = 完整书源 JSON） */
export function saveBookSource(source: BookSource): Promise<ReturnData<null>> {
  return post<null>('/saveBookSource', source)
}

/** POST /reader3/saveBookSources：批量保存（body = 书源数组） */
export function saveBookSources(sources: BookSource[]): Promise<ReturnData<{ count: number }>> {
  return post<{ count: number }>('/saveBookSources', sources)
}

/** POST /reader3/deleteBookSource：删除单个书源（body bookSourceUrl） */
export function deleteBookSource(bookSourceUrl: string): Promise<ReturnData<null>> {
  return post<null>('/deleteBookSource', { bookSourceUrl })
}

/**
 * GET /reader3/getInvalidBookSources：检测失效书源，返回失效书源 URL 列表（string[]）。
 * 后端并行实现中（可能 404）：调用方传 { silent: true } 自行降级提示。
 */
export function getInvalidBookSources(): Promise<ReturnData<string[]>> {
  return get<string[]>('/getInvalidBookSources', undefined, { silent: true })
}
