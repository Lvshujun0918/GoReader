import { post } from './request'
import type { ReturnData, SearchBook } from '@/types'

/** POST /reader3/searchBookMulti：多书源并发搜索（body {key, maxSources}） */
export function searchBookMulti(key: string, maxSources = 50): Promise<ReturnData<SearchBook[]>> {
  return post<SearchBook[]>('/searchBookMulti', { key, maxSources })
}
