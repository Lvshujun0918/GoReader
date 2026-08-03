<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { getBookshelf, saveBook } from '@/api/bookshelf'
import { getBookInfo, searchBookSource } from '@/api/books'
import { searchBookContent } from '@/api/cache'
import { exportBook, type ExportFormat } from '@/api/export'
import { cacheBookOnServer, cacheBookSSE, cancelCacheBook } from '@/api/cacheBook'
import { downloadBlob } from '@/utils/download'
import type { Book, BookInfo, ContentSearchHit, SearchBook } from '@/types'

const route = useRoute()
const router = useRouter()

/** /book/:url —— vue-router 已自动解码 */
const bookUrl = computed(() => String(route.params.url ?? ''))

/** 非书架书的书源信息：入口（搜索结果等）通过 query 传入 */
const queryOrigin = computed(() => String(route.query.origin ?? ''))
const queryOriginName = computed(() => String(route.query.originName ?? ''))

const shelfBook = ref<Book | null>(null)
const info = ref<BookInfo | null>(null)
const loading = ref(true)
const loadFailed = ref(false)
const errorMsg = ref('')
const coverFailed = ref(false)
const saving = ref(false)

/** 展示数据：实时详情优先，书架数据兜底 */
const display = computed(() => ({
  name: info.value?.name || shelfBook.value?.name || '未知书名',
  author: info.value?.author || shelfBook.value?.author || '',
  cover: info.value?.coverUrl || shelfBook.value?.coverUrl || '',
  intro: info.value?.intro || shelfBook.value?.intro || '',
  latestChapterTitle:
    info.value?.latestChapterTitle || shelfBook.value?.latestChapterTitle || '',
}))

function coverInitial(name: string): string {
  const ch = name.trim().charAt(0)
  return ch ? ch.toUpperCase() : '书'
}

/** 本地书（local:// 或文件型 .txt）：后端 local 分支直查书架，不依赖书源 */
function isLocalBookUrl(url: string): boolean {
  return url.startsWith('local://') || url.endsWith('.txt')
}

async function load() {
  loading.value = true
  loadFailed.value = false
  errorMsg.value = ''
  info.value = null
  try {
    // ① 先查书架定位本书
    const res = await getBookshelf()
    const found = (res.data ?? []).find((b) => b.bookUrl === bookUrl.value) ?? null
    shelfBook.value = found

    if (found?.origin) {
      // ② 书架书：详情接口 bookSource=book.origin，实时详情优先，失败用书架数据兜底
      try {
        const infoRes = await getBookInfo(bookUrl.value, found.origin)
        if (infoRes.isSuccess) info.value = infoRes.data
      } catch {
        // 实时详情失败：用书架数据兜底展示
      }
    } else if (isLocalBookUrl(bookUrl.value)) {
      // ③ 本地书：后端 local 分支直查书架返回（无需 bookSource；不在书架则报错）
      try {
        const infoRes = await getBookInfo(bookUrl.value, '')
        if (infoRes.isSuccess) info.value = infoRes.data
      } catch (err) {
        loadFailed.value = true
        errorMsg.value = err instanceof Error ? err.message : '未找到这本书（可能不在书架中）'
      }
    } else if (queryOrigin.value) {
      // ④ 非书架书：直接调详情接口（后端已支持非书架书，bookSource=入口传入的 origin）
      try {
        const infoRes = await getBookInfo(bookUrl.value, queryOrigin.value)
        if (infoRes.isSuccess) info.value = infoRes.data
      } catch (err) {
        loadFailed.value = true
        errorMsg.value = err instanceof Error ? err.message : '获取详情失败'
      }
    } else {
      // ⑤ 非书架书且无书源信息：无法获取详情
      loadFailed.value = true
      errorMsg.value = '未找到这本书（可能不在书架中）'
    }
  } catch {
    loadFailed.value = true
    errorMsg.value = '书架拉取失败，请稍后重试'
  } finally {
    loading.value = false
  }
}

/** 由详情信息组装完整 Book JSON（saveBook 入架 body：type/group 用默认值 0） */
function buildShelfBook(): Book {
  const i = info.value
  return {
    bookUrl: i?.bookUrl || bookUrl.value,
    tocUrl: i?.tocUrl || '',
    origin: i?.origin || queryOrigin.value,
    originName: i?.originName || queryOriginName.value,
    name: i?.name || '',
    author: i?.author || '',
    kind: i?.kind ?? null,
    coverUrl: i?.coverUrl ?? null,
    intro: i?.intro ?? null,
    charset: null,
    type: 0,
    group: 0,
    latestChapterTitle: i?.latestChapterTitle ?? null,
    latestChapterTime: 0,
  }
}

/** 加入书架（非书架书）：POST /reader3/saveBook，成功即视为书架书 */
async function addToShelf() {
  if (saving.value || !info.value) return
  saving.value = true
  try {
    await saveBook(buildShelfBook())
    shelfBook.value = buildShelfBook()
  } catch {
    // 失败提示由 request.ts 统一 toast，按钮保持「加入书架」
  } finally {
    saving.value = false
  }
}

function startReading() {
  if (!shelfBook.value) return
  void router.push(`/reader/${encodeURIComponent(shelfBook.value.bookUrl)}`)
}

/* ================= 全书搜索（GET /reader3/searchBookContent，本地书正文逐章匹配） ================= */

const searchOpen = ref(false)
const searchKey = ref('')
const searchBusy = ref(false)
const searchHits = ref<ContentSearchHit[]>([])
const searchMsg = ref('')
const searchMsgError = ref(false)

function openSearch() {
  searchKey.value = ''
  searchHits.value = []
  searchMsg.value = ''
  searchMsgError.value = false
  searchOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeSearch() {
  if (searchBusy.value) return
  searchOpen.value = false
  document.body.style.overflow = ''
}

async function runSearch() {
  if (searchBusy.value) return
  const key = searchKey.value.trim()
  if (!key) {
    searchMsg.value = '请输入搜索关键词'
    searchMsgError.value = true
    return
  }
  searchBusy.value = true
  searchMsg.value = ''
  searchMsgError.value = false
  searchHits.value = []
  try {
    const res = await searchBookContent(key, bookUrl.value)
    const hits = res.data ?? []
    searchHits.value = hits
    if (hits.length === 0) {
      searchMsg.value = '未找到匹配内容'
      searchMsgError.value = false
    } else {
      searchMsg.value = `共 ${hits.length} 个章节命中`
    }
  } catch (err) {
    // 接口未实现（404）/失败：在弹层内提示，不弹全局 toast
    searchMsg.value =
      err instanceof Error && err.message
        ? `搜索失败：${err.message}`
        : '搜索失败，请稍后重试'
    searchMsgError.value = true
  } finally {
    searchBusy.value = false
  }
}

/** 点击命中 → 跳阅读页并定位到该章（/reader/:bookUrl?chapter=index） */
function goToHit(hit: ContentSearchHit) {
  closeSearch()
  void router.push(`/reader/${encodeURIComponent(bookUrl.value)}?chapter=${hit.chapterIndex}`)
}

/* ================= 换源（GET /reader3/searchBookSource：搜索同书其他书源，点击切换） ================= */

const sourceOpen = ref(false)
const sourceBusy = ref(false)
const sourceSwitching = ref(false)
const sourceResults = ref<SearchBook[]>([])
const sourceMsg = ref('')
const sourceMsgError = ref(false)
const currentOrigin = ref('')

/** 判断是否接口未实现（404 / 后端未就绪） */
function isNotImplemented(err: unknown): boolean {
  const e = err as { response?: { status?: number }; message?: string } | null | undefined
  const status = e?.response?.status
  if (status === 404 || status === 501) return true
  const msg = e?.message ?? ''
  return !e?.response && (msg.includes('404') || msg.includes('Network Error'))
}

/** 书架书且有书源才可换源（本地书无 origin 不显示入口） */
function canSwitchSource(): boolean {
  return !!shelfBook.value && !!shelfBook.value.origin
}

function openSource() {
  sourceOpen.value = true
  document.body.style.overflow = 'hidden'
  void runSourceSearch()
}

function closeSource() {
  if (sourceBusy.value || sourceSwitching.value) return
  sourceOpen.value = false
  document.body.style.overflow = ''
}

/** 搜索同书其他书源：url=当前书 bookUrl + bookSource=当前源 */
async function runSourceSearch() {
  const b = shelfBook.value
  if (!b || !b.origin) return
  sourceBusy.value = true
  sourceResults.value = []
  sourceMsg.value = ''
  sourceMsgError.value = false
  currentOrigin.value = b.origin
  try {
    const res = await searchBookSource(b.bookUrl, b.origin, { silent: true })
    // 按书源去重（同一源多条结果只留首个）
    const seen = new Set<string>()
    sourceResults.value = (res.data ?? []).filter((r) => {
      const k = r.origin || r.originName
      if (!k || seen.has(k)) return false
      seen.add(k)
      return true
    })
    if (sourceResults.value.length === 0) {
      sourceMsg.value = '未找到其他书源'
      sourceMsgError.value = false
    }
  } catch (err) {
    sourceMsg.value = isNotImplemented(err)
      ? '换源搜索接口后端暂未提供（GET /reader3/searchBookSource）'
      : `换源搜索失败：${err instanceof Error ? err.message : '请稍后重试'}`
    sourceMsgError.value = true
  } finally {
    sourceBusy.value = false
  }
}

/** 点击结果 → 切换书源：saveBook 更新 origin/originName/tocUrl（bookUrl 保持书架主键不变） */
async function switchSource(r: SearchBook) {
  const b = shelfBook.value
  if (!b || sourceSwitching.value) return
  if (!r.origin || r.origin === currentOrigin.value) return
  sourceSwitching.value = true
  try {
    await saveBook({
      bookUrl: b.bookUrl,
      origin: r.origin,
      originName: r.originName,
      tocUrl: r.tocUrl,
    } as Book)
    // 本地同步书架条目 + 用新源刷新详情
    b.origin = r.origin
    b.originName = r.originName
    b.tocUrl = r.tocUrl
    currentOrigin.value = r.origin
    info.value = null
    try {
      const infoRes = await getBookInfo(bookUrl.value, r.origin)
      if (infoRes.isSuccess) info.value = infoRes.data
    } catch {
      // 详情刷新失败：书架数据兜底展示
    }
    ElMessage.success(`已切换到「${r.originName || r.origin}」`)
    closeSource()
  } catch {
    // 失败提示由 request.ts 统一 toast（saveBook 非 silent）
  } finally {
    sourceSwitching.value = false
  }
}

/* ================= 导出（GET /reader3/exportBook：txt/epub/html blob 下载） ================= */

const EXPORT_FORMATS: { value: ExportFormat; label: string; tip: string }[] = [
  { value: 'txt', label: 'TXT', tip: '纯文本' },
  { value: 'epub', label: 'EPUB', tip: '电子书' },
  { value: 'html', label: 'HTML', tip: '网页' },
]

const exportOpen = ref(false)
const exportFormat = ref<ExportFormat>('txt')
const exportBusy = ref(false)
const exportMsg = ref('')
const exportMsgError = ref(false)

function openExport() {
  exportFormat.value = 'txt'
  exportMsg.value = ''
  exportMsgError.value = false
  exportOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeExport() {
  if (exportBusy.value) return
  exportOpen.value = false
  document.body.style.overflow = ''
}

/** 导出并下载（失败在弹窗内提示，不弹全局 toast——接口可能未实现） */
async function confirmExport() {
  if (exportBusy.value) return
  exportBusy.value = true
  exportMsg.value = ''
  exportMsgError.value = false
  try {
    const blob = await exportBook(bookUrl.value, exportFormat.value)
    const name = `${(display.value.name || 'book').replace(/[\\/:*?"<>|]/g, '_')}.${exportFormat.value}`
    const ok = await downloadBlob(blob, name)
    if (ok) {
      exportMsg.value = `已下载 ${name}`
      window.setTimeout(() => {
        if (!exportBusy.value) closeExport()
      }, 900)
    }
  } catch (err) {
    exportMsg.value = isNotImplemented(err)
      ? '导出接口后端暂未提供（GET /reader3/exportBook）'
      : `导出失败：${err instanceof Error ? err.message : '请稍后重试'}`
    exportMsgError.value = true
  } finally {
    exportBusy.value = false
  }
}

/* ================= 缓存本书（POST /reader3/cacheBookOnServer：SSE 进度 cached/total） ================= */

const cacheOpen = ref(false)
const cacheBusy = ref(false)
const cacheDone = ref(false)
const cacheCached = ref(0)
const cacheTotal = ref(0)
const cacheTitle = ref('')
const cacheMsg = ref('')
const cacheMsgError = ref(false)
let cacheHandle: { close: () => void } | null = null

const cachePercent = computed(() => {
  if (cacheTotal.value <= 0) return cacheCached.value > 0 ? 100 : 0
  return Math.min(100, Math.round((cacheCached.value / cacheTotal.value) * 100))
})

function openCache() {
  cacheOpen.value = true
  cacheBusy.value = false
  cacheDone.value = false
  cacheCached.value = 0
  cacheTotal.value = 0
  cacheTitle.value = ''
  cacheMsg.value = ''
  cacheMsgError.value = false
  document.body.style.overflow = 'hidden'
  void startCache()
}

function closeCache() {
  if (cacheBusy.value) {
    cacheHandle?.close()
    cacheHandle = null
    cacheBusy.value = false
  }
  cacheOpen.value = false
  document.body.style.overflow = ''
}

async function startCache() {
  cacheBusy.value = true
  cacheDone.value = false
  cacheMsg.value = ''
  cacheMsgError.value = false
  try {
    // ① POST 启动后台缓存任务（后端立即返回 {started,cached,total,title}）
    const res = await cacheBookOnServer(bookUrl.value)
    if (!res.isSuccess) throw new Error(res.errorMsg || '缓存启动失败')
    const start = res.data
    if (start) {
      cacheCached.value = start.cached ?? 0
      cacheTotal.value = start.total ?? 0
      if (start.title) cacheTitle.value = start.title
    }
    // ② 订阅进度流（GET /reader3/cacheBookSSE：cached/total/title/finished/cancelled/error）
    const handle = await cacheBookSSE(bookUrl.value, {
      onProgress: (p) => {
        cacheCached.value = p.cached
        cacheTotal.value = p.total
        if (p.title) cacheTitle.value = p.title
        if (p.cancelled) {
          cacheBusy.value = false
          cacheMsg.value = '缓存已取消'
          cacheMsgError.value = false
        } else if (p.error) {
          cacheBusy.value = false
          cacheMsg.value = `缓存失败：${p.error}`
          cacheMsgError.value = true
        } else if (p.finished) {
          cacheBusy.value = false
          cacheDone.value = true
          cacheMsg.value = '缓存完成，可随时离线/多端阅读'
          cacheMsgError.value = false
        }
      },
      onEnd: () => {
        cacheBusy.value = false
        if (!cacheMsg.value) {
          cacheDone.value = true
          cacheMsg.value = '缓存完成，可随时离线/多端阅读'
        }
      },
      onStreamError: (msg) => {
        cacheBusy.value = false
        cacheMsg.value = `缓存进度中断：${msg}`
        cacheMsgError.value = true
      },
    })
    cacheHandle = handle
  } catch (err) {
    cacheBusy.value = false
    cacheMsg.value = isNotImplemented(err)
      ? '缓存接口后端暂未提供（POST /reader3/cacheBookOnServer）'
      : `缓存失败：${err instanceof Error ? err.message : '请稍后重试'}`
    cacheMsgError.value = true
  }
}

/** 取消缓存：中断 SSE 流 + 通知后端（GET /reader3/cancelCacheBook，静默降级） */
async function cancelCache() {
  if (!cacheBusy.value) return
  cacheHandle?.close()
  cacheHandle = null
  cacheBusy.value = false
  try {
    await cancelCacheBook(bookUrl.value)
  } catch {
    // 接口未实现：忽略（本地流已中断）
  }
  cacheMsg.value = '已取消缓存'
  cacheMsgError.value = false
}

onMounted(load)
</script>

<template>
  <div class="detail-page">
    <!-- 极简顶栏：返回书架 -->
    <header class="topbar">
      <button class="back-btn" type="button" @click="router.push('/')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5" />
          <path d="M11 18l-6-6 6-6" />
        </svg>
        <span>书架</span>
      </button>
      <span class="brand">夜读<span class="brand-dot">.</span></span>
    </header>

    <main class="content">
      <!-- 加载骨架（浅灰静置块） -->
      <div v-if="loading" class="detail-layout" aria-label="加载中">
        <div class="skeleton-cover"></div>
        <div class="skeleton-info">
          <div class="skeleton-line wide"></div>
          <div class="skeleton-line"></div>
          <div class="skeleton-line"></div>
          <div class="skeleton-line short"></div>
        </div>
      </div>

      <!-- 错误态：不在书架 / 书源获取失败 / 书架拉取失败 -->
      <div v-else-if="loadFailed" class="empty-state">
        <p class="empty-text">{{ errorMsg || '未找到这本书（可能不在书架中）' }}</p>
        <div class="empty-actions">
          <button class="ghost-btn" type="button" @click="load">重试</button>
          <button class="ghost-btn" type="button" @click="router.push('/')">返回书架</button>
        </div>
      </div>

      <!-- 详情 -->
      <div v-else class="detail-layout">
        <!-- 封面 -->
        <div class="cover-wrap">
          <img
            v-if="display.cover && !coverFailed"
            :src="display.cover"
            class="cover-img"
            :alt="display.name"
            @error="coverFailed = true"
          />
          <div v-else class="cover-ph">
            <span class="cover-ph-char">{{ coverInitial(display.name) }}</span>
          </div>
        </div>

        <!-- 信息 -->
        <div class="book-info">
          <h1 class="book-name">{{ display.name }}</h1>
          <p v-if="display.author" class="book-author">{{ display.author }}</p>
          <p v-if="display.latestChapterTitle" class="book-latest">
            最新章节：{{ display.latestChapterTitle }}
          </p>

          <p v-if="display.intro" class="book-intro">{{ display.intro }}</p>

          <div class="actions">
            <!-- 书架书 → 开始阅读；非书架书 → 加入书架（入架成功后变开始阅读） -->
            <button v-if="shelfBook" class="read-btn" type="button" @click="startReading">开始阅读</button>
            <button v-else class="add-btn" type="button" :disabled="saving" @click="addToShelf">
              加入书架
            </button>
            <!-- 全书搜索（书架书本地正文搜索；命中后跳阅读页该章） -->
            <button v-if="shelfBook" class="search-btn" type="button" @click="openSearch">全书搜索</button>
            <!-- 换源（书架书且带书源：搜索同书其他书源并切换） -->
            <button v-if="canSwitchSource()" class="search-btn" type="button" @click="openSource">换源</button>
            <!-- 导出（GET /reader3/exportBook：txt/epub/html blob 下载） -->
            <button class="search-btn" type="button" @click="openExport">导出</button>
            <!-- 缓存本书（POST /reader3/cacheBookOnServer：SSE 进度条） -->
            <button class="search-btn" type="button" @click="openCache">缓存本书</button>
          </div>
        </div>
      </div>
    </main>

    <!-- 全书搜索弹层（GET /reader3/searchBookContent） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="searchOpen" class="dlg-overlay" @click.self="closeSearch">
          <div
            class="dlg dlg-search"
            role="dialog"
            aria-modal="true"
            aria-label="全书搜索"
            tabindex="-1"
            @keydown.esc="closeSearch"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">全书搜索</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="searchBusy" @click="closeSearch">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <form class="dlg-form" @submit.prevent="runSearch">
              <div class="search-row">
                <input
                  v-model="searchKey"
                  class="search-input"
                  type="text"
                  placeholder="搜索《{{ display.name }}》正文"
                  spellcheck="false"
                />
                <button class="accent-btn" type="submit" :disabled="searchBusy || !searchKey.trim()">
                  {{ searchBusy ? '搜索中…' : '搜索' }}
                </button>
              </div>
              <p class="field-tip">搜索本书全部章节正文（本地书），命中后点击跳转阅读页对应章节。</p>
              <p v-if="searchMsg" class="search-msg" :class="{ error: searchMsgError }">{{ searchMsg }}</p>
              <ul v-if="searchHits.length" class="search-hits">
                <li v-for="(hit, i) in searchHits" :key="`${hit.chapterIndex}-${i}`">
                  <button class="hit-btn" type="button" @click="goToHit(hit)">
                    <span class="hit-title">{{ hit.title || `第 ${hit.chapterIndex + 1} 章` }}</span>
                    <span class="hit-snippet">{{ hit.snippet }}</span>
                  </button>
                </li>
              </ul>
            </form>
          </div>
        </div>
      </Transition>
    </Teleport>
    <!-- 换源弹层（GET /reader3/searchBookSource：搜索同书其他书源，点击切换） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="sourceOpen" class="dlg-overlay" @click.self="closeSource">
          <div
            class="dlg dlg-source"
            role="dialog"
            aria-modal="true"
            aria-label="换源"
            tabindex="-1"
            @keydown.esc="closeSource"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">换源 · {{ display.name }}</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="sourceBusy || sourceSwitching" @click="closeSource">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <div class="source-body">
              <p class="field-tip">搜索《{{ display.name }}》的其他书源（当前：{{ currentOrigin || '—' }}），点击结果即可切换。</p>

              <!-- 搜索中 -->
              <div v-if="sourceBusy" class="source-busy">
                <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                  <path d="M21 12a9 9 0 1 1-6.2-8.56" />
                </svg>
                <span>正在搜索其他书源…</span>
              </div>

              <!-- 结果列表：书源名 + 来源，当前源置灰标记 -->
              <ul v-else-if="sourceResults.length" class="source-list">
                <li v-for="(r, i) in sourceResults" :key="i">
                  <button
                    class="source-row"
                    type="button"
                    :disabled="sourceSwitching || r.origin === currentOrigin"
                    :title="r.origin === currentOrigin ? '当前书源' : '切换到该书源'"
                    @click="switchSource(r)"
                  >
                    <span class="source-name">{{ r.originName || r.origin || '未知书源' }}</span>
                    <span class="source-url">{{ r.origin }}</span>
                    <span v-if="r.origin === currentOrigin" class="source-cur">当前</span>
                  </button>
                </li>
              </ul>

              <!-- 空 / 失败提示 -->
              <template v-else>
                <p v-if="sourceMsg" class="search-msg" :class="{ error: sourceMsgError }">{{ sourceMsg }}</p>
                <div v-if="sourceMsgError" class="source-retry">
                  <button class="ghost-btn" type="button" @click="runSourceSearch">重试</button>
                </div>
              </template>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
    <!-- 导出弹层（GET /reader3/exportBook：txt/epub/html） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="exportOpen" class="dlg-overlay" @click.self="closeExport">
          <div
            class="dlg dlg-export"
            role="dialog"
            aria-modal="true"
            aria-label="导出书籍"
            tabindex="-1"
            @keydown.esc="closeExport"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">导出 · {{ display.name }}</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="exportBusy" @click="closeExport">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <div class="export-formats">
              <button
                v-for="f in EXPORT_FORMATS"
                :key="f.value"
                class="fmt-btn"
                :class="{ active: exportFormat === f.value }"
                type="button"
                :disabled="exportBusy"
                @click="exportFormat = f.value"
              >
                <span class="fmt-label">{{ f.label }}</span>
                <span class="fmt-tip">{{ f.tip }}</span>
              </button>
            </div>
            <p class="field-tip">由服务器生成 {{ exportFormat.toUpperCase() }} 文件并下载。</p>
            <p v-if="exportMsg" class="search-msg" :class="{ error: exportMsgError }">{{ exportMsg }}</p>
            <div class="dlg-actions">
              <button class="ghost-btn" type="button" :disabled="exportBusy" @click="closeExport">取消</button>
              <button class="accent-btn" type="button" :disabled="exportBusy" @click="confirmExport">
                {{ exportBusy ? '导出中…' : '导出' }}
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 缓存本书弹层（POST /reader3/cacheBookOnServer：SSE 进度 cached/total） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="cacheOpen" class="dlg-overlay" @click.self="closeCache">
          <div
            class="dlg dlg-cache"
            role="dialog"
            aria-modal="true"
            aria-label="缓存本书"
            tabindex="-1"
            @keydown.esc="closeCache"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">缓存本书{{ cacheTitle ? ' · ' + cacheTitle : '' }}</h2>
              <button class="dlg-close" type="button" title="关闭" @click="closeCache">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <p class="field-tip">将本书章节缓存到服务器，之后可在其他设备快速阅读。</p>
            <div class="cache-progress">
              <div class="cache-bar">
                <div class="cache-fill" :style="{ width: cachePercent + '%' }"></div>
              </div>
              <span class="cache-percent">
                {{ cacheBusy ? `${cacheCached} / ${cacheTotal}` : cacheDone ? '完成' : `${cachePercent}%` }}
              </span>
            </div>
            <p v-if="cacheMsg" class="search-msg" :class="{ error: cacheMsgError }">{{ cacheMsg }}</p>
            <div class="dlg-actions">
              <button v-if="cacheBusy" class="ghost-btn" type="button" @click="cancelCache">取消缓存</button>
              <template v-else>
                <button v-if="cacheMsgError" class="ghost-btn" type="button" @click="startCache">重试</button>
                <button class="accent-btn" type="button" @click="closeCache">关闭</button>
              </template>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.detail-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  animation: fade-in 0.2s ease both;
}

/* ================= 顶栏 ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 32px;
  background: rgba(250, 250, 250, 0.86);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}
.back-btn {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px 6px 8px;
  border: 1px solid transparent;
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.back-btn:hover {
  color: var(--text-1);
  border-color: var(--border);
}
.back-btn svg {
  width: 15px;
  height: 15px;
}
.brand {
  font-size: 15px;
  font-weight: 300;
  letter-spacing: 3px;
  color: var(--text-1);
}
.brand-dot {
  color: var(--accent);
  font-weight: 400;
}

/* ================= 内容 ================= */
.content {
  flex: 1;
  width: min(860px, 100%);
  margin: 0 auto;
  padding: 56px 32px 80px;
}

.detail-layout {
  display: grid;
  grid-template-columns: 220px 1fr;
  gap: 48px;
  align-items: start;
}

/* 封面 */
.cover-wrap {
  position: relative;
  aspect-ratio: 3 / 4;
  width: 220px;
  border-radius: 10px;
  overflow: hidden;
  border: 1px solid var(--border);
  background: var(--surface);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}
.cover-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.cover-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #9aa8a0;
}
.cover-ph-char {
  font-size: 56px;
  font-weight: 300;
  color: rgba(255, 255, 255, 0.94);
  letter-spacing: 2px;
}

/* 信息 */
.book-info {
  padding-top: 4px;
}
.book-name {
  margin: 0;
  font-size: 32px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-1);
  line-height: 1.4;
}
.book-author {
  margin: 14px 0 0;
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.book-latest {
  margin: 10px 0 0;
  font-size: 13px;
  font-weight: 300;
  color: var(--text-3);
  max-width: 520px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-latest::before {
  content: '';
  display: inline-block;
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: var(--accent);
  margin-right: 8px;
  vertical-align: 2px;
}

/* 简介：留白段落 */
.book-intro {
  margin: 28px 0 0;
  max-width: 560px;
  font-size: 14px;
  font-weight: 300;
  line-height: 2;
  letter-spacing: 0.5px;
  color: var(--text-2);
  white-space: pre-line;
}

/* 操作区 */
.actions {
  margin-top: 40px;
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
}
.read-btn {
  padding: 13px 44px;
  border: none;
  border-radius: var(--radius);
  background: var(--accent);
  color: #fff;
  font-family: inherit;
  font-size: 14.5px;
  font-weight: 400;
  letter-spacing: 4px;
  cursor: pointer;
  transition: background 0.2s ease;
}
.read-btn:hover {
  background: var(--accent-deep);
}
.read-btn:active {
  background: var(--accent-deep);
}

/* 加入书架：细字描边 → hover 强调色 */
.add-btn {
  padding: 13px 44px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 14.5px;
  font-weight: 300;
  letter-spacing: 4px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.add-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.add-btn:disabled {
  opacity: 0.55;
  cursor: default;
}

/* 全书搜索：细字描边按钮（次于主按钮） */
.search-btn {
  padding: 13px 32px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 3px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.search-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
}

/* ================= 骨架 / 空态 ================= */
.skeleton-cover {
  width: 220px;
  aspect-ratio: 3 / 4;
  border-radius: 10px;
  background: #f0f0f2;
  border: 1px solid var(--border);
}
.skeleton-info {
  padding-top: 8px;
}
.skeleton-line {
  height: 13px;
  margin-bottom: 18px;
  border-radius: 4px;
  background: #f0f0f2;
}
.skeleton-line.wide {
  width: 60%;
  height: 26px;
}
.skeleton-line.short {
  width: 40%;
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 24px;
  padding: 120px 0;
}
.empty-text {
  margin: 0;
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.empty-actions {
  display: flex;
  align-items: center;
  gap: 16px;
}
.ghost-btn {
  padding: 9px 28px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 2px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.ghost-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
}

/* ================= 全书搜索弹层 ================= */
.dlg-overlay {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(24, 24, 27, 0.35);
}
.dlg {
  width: min(520px, 100%);
  max-height: min(560px, 86vh);
  display: flex;
  flex-direction: column;
  padding: 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.08);
  outline: none;
}
.dlg-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;
}
.dlg-title {
  margin: 0;
  font-size: 15px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--text-1);
}
.dlg-close {
  width: 26px;
  height: 26px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease;
}
.dlg-close:hover:not(:disabled) {
  color: var(--text-1);
  background: #f4f4f5;
}
.dlg-close:disabled {
  cursor: not-allowed;
  opacity: 0.4;
}
.dlg-close svg {
  width: 13px;
  height: 13px;
}
.dlg-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
}
.search-row {
  display: flex;
  gap: 8px;
}
.search-input {
  flex: 1;
  min-width: 0;
  height: 36px;
  padding: 0 12px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--bg);
  color: var(--text-1);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  outline: none;
  transition: border-color 0.2s ease;
}
.search-input::placeholder {
  color: var(--text-3);
  font-weight: 300;
}
.search-input:focus {
  border-color: var(--accent);
  background: var(--surface);
}
.accent-btn {
  flex-shrink: 0;
  padding: 0 20px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #ffffff;
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    background-color 0.2s ease,
    border-color 0.2s ease;
}
.accent-btn:hover:not(:disabled) {
  background: var(--accent-deep);
  border-color: var(--accent-deep);
}
.accent-btn:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}
.field-tip {
  margin: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.search-msg {
  margin: 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-2);
}
.search-msg.error {
  color: #cf4444;
}
.search-hits {
  list-style: none;
  margin: 4px 0 0;
  padding: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.hit-btn {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 9px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg);
  text-align: left;
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.hit-btn:hover {
  border-color: var(--accent);
  background: var(--accent-soft);
}
.hit-title {
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.hit-snippet {
  font-size: 12px;
  font-weight: 300;
  line-height: 1.6;
  color: var(--text-3);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* ================= 换源弹层 ================= */
.dlg-source {
  width: min(480px, 100%);
}
.source-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
  min-height: 0;
  overflow-y: auto;
}
.source-busy {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 18px 4px;
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
}
.mini-spin {
  width: 13px;
  height: 13px;
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
.source-list {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.source-row {
  width: 100%;
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg);
  text-align: left;
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.source-row:hover:not(:disabled) {
  border-color: var(--accent);
  background: var(--accent-soft);
}
.source-row:disabled {
  cursor: default;
  opacity: 0.6;
}
.source-name {
  flex-shrink: 0;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
}
.source-url {
  flex: 1;
  min-width: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.source-cur {
  flex-shrink: 0;
  padding: 1px 7px;
  border-radius: 999px;
  border: 1px solid var(--accent);
  color: var(--accent);
  font-size: 10.5px;
  font-weight: 400;
  letter-spacing: 1px;
}
.source-retry {
  display: flex;
  justify-content: flex-start;
}

/* ================= 导出弹层 ================= */
.dlg-export {
  width: min(420px, 100%);
}
.export-formats {
  display: flex;
  gap: 8px;
  margin-bottom: 10px;
}
.fmt-btn {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;
  padding: 10px 0;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--bg);
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.fmt-btn:hover:not(:disabled) {
  border-color: var(--accent);
}
.fmt-btn.active {
  border-color: var(--accent);
  background: var(--accent-soft);
}
.fmt-btn:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.fmt-label {
  font-size: 13.5px;
  font-weight: 400;
  letter-spacing: 2px;
  color: var(--text-1);
}
.fmt-tip {
  font-size: 10.5px;
  font-weight: 300;
  color: var(--text-3);
}
.dlg-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}
.ghost-btn {
  padding: 7px 18px;
  border-radius: var(--radius);
  border: 1px solid var(--border-strong);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.ghost-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.ghost-btn:disabled,
.accent-btn:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}
.accent-btn {
  padding: 7px 18px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #ffffff;
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    background-color 0.2s ease,
    border-color 0.2s ease;
}
.accent-btn:hover:not(:disabled) {
  background: var(--accent-deep);
  border-color: var(--accent-deep);
}

/* ================= 缓存本书弹层 ================= */
.dlg-cache {
  width: min(420px, 100%);
}
.cache-progress {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 12px 0 2px;
}
.cache-bar {
  flex: 1;
  height: 5px;
  border-radius: 999px;
  background: var(--hover);
  overflow: hidden;
}
.cache-fill {
  height: 100%;
  border-radius: 999px;
  background: var(--accent);
  transition: width 0.3s ease;
}
.cache-percent {
  flex-shrink: 0;
  min-width: 56px;
  text-align: right;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
  font-variant-numeric: tabular-nums;
}

/* 弹窗动画：fade 200ms */
.dlg-enter-active,
.dlg-leave-active {
  transition: opacity 0.2s ease;
}
.dlg-enter-from,
.dlg-leave-to {
  opacity: 0;
}
.dlg-enter-active .dlg,
.dlg-leave-active .dlg {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}
.dlg-enter-from .dlg,
.dlg-leave-to .dlg {
  opacity: 0;
  transform: translateY(6px);
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    padding: 12px 16px;
  }
  .content {
    padding: 36px 20px 64px;
  }
  .detail-layout {
    grid-template-columns: 1fr;
    gap: 28px;
  }
  .cover-wrap,
  .skeleton-cover {
    width: 168px;
  }
  .book-name {
    font-size: 26px;
  }
}
</style>
