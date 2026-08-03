import { get, post } from './request'
import type { Book, BookGroup, ReturnData } from '@/types'

/** GET /reader3/getBookshelf */
export function getBookshelf(): Promise<ReturnData<Book[]>> {
  return get<Book[]>('/getBookshelf')
}

/** POST /reader3/saveBook：入架/编辑（body = 完整 Book JSON，upsert） */
export function saveBook(book: Book): Promise<ReturnData<null>> {
  return post<null>('/saveBook', book)
}

/** POST /reader3/deleteBook：移出书架（bookUrl） */
export function deleteBook(bookUrl: string): Promise<ReturnData<null>> {
  return post<null>('/deleteBook', { bookUrl })
}

/** GET /reader3/getBookGroups：书架分组列表 */
export function getBookGroups(): Promise<ReturnData<BookGroup[]>> {
  return get<BookGroup[]>('/getBookGroups')
}

/** POST /reader3/saveBookGroup：新建分组（body 传 {name} 即可，id<=0 自动新建） */
export function saveBookGroup(name: string): Promise<ReturnData<BookGroup>> {
  return post<BookGroup>('/saveBookGroup', { name })
}

/**
 * POST /reader3/updateBookGroupId：书设分组
 * 注意：后端实现读 body/query 的 group（数值分组 id）写入 books.group_name，
 * 并非字面上的 groupId 字段。
 */
export function updateBookGroupId(bookUrl: string, group: number): Promise<ReturnData<null>> {
  return post<null>('/updateBookGroupId', { bookUrl, group })
}
