import { get } from './request'
import type { BookChapter, BookContent, BookInfo, ReturnData, SearchBook } from '@/types'

/** GET /reader3/getBookInfo：书籍详情（参数 url + bookSource=book.origin） */
export function getBookInfo(url: string, bookSource: string, opts?: { silent?: boolean }): Promise<ReturnData<BookInfo>> {
  return get<BookInfo>('/getBookInfo', { url, bookSource }, opts)
}

/**
 * GET /reader3/searchBookSource：换源搜索——按 url（当前书 bookUrl）+ bookSource（当前源）
 * 搜索同书的其他书源，返回 SearchBook[]（每项含新源 origin/originName/tocUrl）。
 * 后端并行实现中（可能 404）：调用方传 { silent: true } 自行降级提示。
 */
export function searchBookSource(
  url: string,
  bookSource: string,
  opts?: { silent?: boolean },
): Promise<ReturnData<SearchBook[]>> {
  return get<SearchBook[]>('/searchBookSource', { url, bookSource }, opts)
}

/** GET /reader3/getBookToc：章节目录（tocUrl=info.tocUrl + bookSource） */
export function getBookToc(tocUrl: string, bookSource: string): Promise<ReturnData<BookChapter[]>> {
  return get<BookChapter[]>('/getBookToc', { tocUrl, bookSource })
}

/** GET /reader3/getBookContent：章节正文（chapterUrl + bookSource，正文在 data.content） */
export function getBookContent(chapterUrl: string, bookSource: string): Promise<ReturnData<BookContent>> {
  return get<BookContent>('/getBookContent', { chapterUrl, bookSource })
}
