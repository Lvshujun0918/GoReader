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

/** 搜索结果（/reader3/searchBookMulti → SearchBook，全字段 camelCase） */
export interface SearchBook {
  bookUrl: string
  origin: string
  originName: string
  type: number
  name: string
  author: string
  kind?: string | null
  coverUrl?: string | null
  intro?: string | null
  wordCount?: string | null
  latestChapterTitle?: string | null
  tocUrl: string
  time?: number
  variable?: string | null
  originOrder?: number
  [key: string]: unknown
}

/** 书架分组（/reader3/getBookGroups → BookGroup，camelCase；books.group 存分组 id，0=未分组） */
export interface BookGroup {
  id: number
  name: string
  order: number
  [key: string]: unknown
}

/** RSS 订阅源（/reader3/getRssSources → RssSource，legacy 兼容 camelCase） */
export interface RssSource {
  rssSourceUrl: string
  rssSourceName: string
  rssSourceGroup?: string | null
  enabled: boolean
  [key: string]: unknown
}

/** RSS 文章（/reader3/getRssArticles → data 数组；content 为正文 HTML，getRssArticle 单独拉取） */
export interface RssArticle {
  url: string
  title: string
  author?: string | null
  time: number
  content?: string | null
  cover?: string | null
  [key: string]: unknown
}

/** 书源（/reader3/getBookSources → BookSource，legado 兼容 camelCase） */
export interface BookSource {
  bookSourceUrl: string
  bookSourceName: string
  bookSourceGroup?: string | null
  bookSourceType: number
  bookUrlPattern?: string | null
  customOrder: number
  enabled: boolean
  enabledExplore: boolean
  enabledCookieJar?: boolean | null
  concurrentRate?: string | null
  header?: string | null
  loginUrl?: string | null
  loginUi?: string | null
  loginCheckJs?: string | null
  loginJs?: string | null
  bookSourceComment?: string | null
  variableComment?: string | null
  lastUpdateTime: number
  respondTime: number
  weight: number
  exploreUrl?: string | null
  searchUrl?: string | null
  [key: string]: unknown
}
