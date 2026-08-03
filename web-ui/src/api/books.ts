import { get } from './request'
import type { BookChapter, BookContent, BookInfo, ReturnData } from '@/types'

/** GET /reader3/getBookInfo：书籍详情（参数 url + bookSource=book.origin） */
export function getBookInfo(url: string, bookSource: string): Promise<ReturnData<BookInfo>> {
  return get<BookInfo>('/getBookInfo', { url, bookSource })
}

/** GET /reader3/getBookToc：章节目录（tocUrl=info.tocUrl + bookSource） */
export function getBookToc(tocUrl: string, bookSource: string): Promise<ReturnData<BookChapter[]>> {
  return get<BookChapter[]>('/getBookToc', { tocUrl, bookSource })
}

/** GET /reader3/getBookContent：章节正文（chapterUrl + bookSource，正文在 data.content） */
export function getBookContent(chapterUrl: string, bookSource: string): Promise<ReturnData<BookContent>> {
  return get<BookContent>('/getBookContent', { chapterUrl, bookSource })
}
