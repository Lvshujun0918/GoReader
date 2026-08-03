import { get } from './request'
import type { Book, ReturnData } from '@/types'

/** GET /reader3/getBookshelf */
export function getBookshelf(): Promise<ReturnData<Book[]>> {
  return get<Book[]>('/getBookshelf')
}
