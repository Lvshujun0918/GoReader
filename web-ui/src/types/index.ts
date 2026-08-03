/** 后端统一返回结构（兼容 legacy ReturnData：isSuccess/errorMsg/data） */
export interface ReturnData<T = unknown> {
  isSuccess: boolean
  errorMsg: string
  data: T
}

/** 登录/注册返回（formatUser，camelCase） */
export interface UserInfo {
  username: string
  lastLoginAt: number
  accessToken: string
  [key: string]: unknown
}

/** 书架书籍（books 表 ↔ /reader3/getBookshelf 输出，全字段 camelCase） */
export interface Book {
  bookUrl: string
  tocUrl: string
  origin: string
  originName: string
  name: string
  author: string
  kind?: string | null
  customTag?: string | null
  coverUrl?: string | null
  customCoverUrl?: string | null
  intro?: string | null
  customIntro?: string | null
  charset?: string | null
  type: number
  group: number
  latestChapterTitle?: string | null
  latestChapterTime: number
  lastCheckTime?: number
  [key: string]: unknown
}

/** 书籍详情（/reader3/getBookInfo → ruleBookInfo，全字段 camelCase） */
export interface BookInfo {
  name: string
  author: string
  kind?: string | null
  intro?: string | null
  coverUrl?: string | null
  tocUrl: string
  wordCount?: string | null
  latestChapterTitle?: string | null
  bookUrl: string
  origin: string
  originName: string
  [key: string]: unknown
}

/** 章节（/reader3/getBookToc → ruleToc，camelCase；isVolume=卷标题分隔行） */
export interface BookChapter {
  title: string
  url: string
  isVolume: boolean
  index: number
  [key: string]: unknown
}

/** 章节正文（/reader3/getBookContent → data.content 纯文本） */
export interface BookContent {
  content: string
}
