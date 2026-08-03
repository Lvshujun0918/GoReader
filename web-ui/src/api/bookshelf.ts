import { get, post } from './request'
import type { Book, ReturnData } from '@/types'

/** GET /reader3/getBookshelf */
export function getBookshelf(): Promise<ReturnData<Book[]>> {
  return get<Book[]>('/getBookshelf')
}

/** POST /reader3/deleteBook：移出书架（bookUrl） */
export function deleteBook(bookUrl: string): Promise<ReturnData<null>> {
  return post<null>('/deleteBook', { bookUrl })
}
