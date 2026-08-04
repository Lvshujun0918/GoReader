import { get, post } from './request'
import type { Bookmark, ReturnData } from '@/types'

/**
 * 书签（/reader3/bookmarks 系列）：
 * - GET  /reader3/getBookmarks?bookUrl=…  → ReturnData<Bookmark[]>
 * - POST /reader3/deleteBookmark          body {bookUrl, title}
 * 后端无跨书批量接口：跨书书签列表（GAP 89）由调用方逐书 getBookmarks 汇总。
 */

/** GET /reader3/getBookmarks：单书书签列表（bookUrl 参数） */
export function getBookmarks(bookUrl: string, opts?: { silent?: boolean }): Promise<ReturnData<Bookmark[]>> {
  return get<Bookmark[]>('/getBookmarks', { bookUrl }, opts)
}

/** POST /reader3/deleteBookmark：删除书签（body：bookUrl + title） */
export function deleteBookmark(bookUrl: string, title: string): Promise<ReturnData<null>> {
  return post<null>('/deleteBookmark', { bookUrl, title })
}
