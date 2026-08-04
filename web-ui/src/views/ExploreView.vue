<template>
  <div class="page">
    <!-- 顶栏 -->
    <header class="topbar">
      <button class="back-btn" type="button" @click="source ? backToSources() : router.push('/')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5" /><path d="M11 18l-6-6 6-6" />
        </svg>
      </button>
      <span class="brand">夜读<em>.</em></span>
      <span class="title">{{ source ? applyHan(source.bookSourceName, hanMode) : '探索' }}</span>
      <div class="top-actions">
        <button class="han-btn" type="button" :title="'简繁转换（当前：' + hanLabel + '）'" @click="toggleHan(); updateHanLabel()">
          {{ hanLabel }}
        </button>
        <button class="han-btn" type="button" title="内容宽度（与阅读页同一设置）" @click="cycleWidth">
          {{ WIDTH_OPTIONS.find((o) => o.value === contentWidth)?.label ?? '适中' }}
        </button>
      </div>
    </header>

    <!-- 书源列表（legado 语义：所有 enabledExplore 书源） -->
    <main v-if="!source" class="main" :style="{ maxWidth: contentWidth }">
      <p class="page-hint">书源探索</p>
      <!-- 搜索框：下拉提示最近搜索（localStorage 最近 10 条，与搜索页共用） -->
      <div class="search-wrap">
        <div class="explore-search">
          <svg
            class="search-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
          >
            <circle cx="11" cy="11" r="6.5" />
            <path d="M20 20l-3.8-3.8" />
          </svg>
          <input
            v-model="searchKey"
            class="search-input"
            type="text"
            placeholder="搜索书籍…"
            spellcheck="false"
            @focus="showDropdown = true"
            @input="showDropdown = true"
            @keydown.enter="onSearchEnter"
            @keydown.esc="showDropdown = false"
          />
          <button
            class="search-go"
            type="button"
            :disabled="!searchKey.trim()"
            @click="onSearchEnter"
          >
            搜索
          </button>
        </div>
        <!-- 下拉提示（最近搜索；点击回填） -->
        <div v-if="showDropdown && suggestions.length" class="search-dropdown">
          <div class="dropdown-head">
            <span class="dropdown-label">最近搜索</span>
            <button class="dropdown-clear" type="button" @click="clearHistory">清空</button>
          </div>
          <ul class="dropdown-list">
            <li
              v-for="h in suggestions"
              :key="h"
              class="dropdown-item"
              @mousedown.prevent="searchKey = h"
            >
              <svg class="item-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                <path d="M5 12h14M13 6l6 6-6 6" />
              </svg>
              <span class="item-text" :title="h">{{ h }}</span>
            </li>
          </ul>
        </div>
      </div>
      <div v-if="sourcesLoading" class="state">
        <p class="state-text">加载中…</p>
      </div>
      <p v-else-if="sourcesError" class="state-error">{{ sourcesError }} <button class="retry" type="button" @click="loadSources">重试</button></p>
      <div v-else-if="sources.length === 0" class="state">
        <p class="state-text">没有支持探索的书源</p>
        <router-link class="link" to="/sources">前往书源管理</router-link>
      </div>
      <ul v-else class="source-list">
        <li v-for="s in sources" :key="s.bookSourceUrl" class="source-item" @click="selectSource(s)">
          <span class="source-name">{{ applyHan(s.bookSourceName, hanMode) }}</span>
          <span class="source-count">{{ exploreCount(s) }} 个分类</span>
          <span class="chevron">›</span>
        </li>
      </ul>
    </main>

    <!-- 书源探索页：分类 + 书籍 -->
    <main v-else class="main" :style="{ maxWidth: contentWidth }">
      <div v-if="catsLoading" class="state"><p class="state-text">加载分类…</p></div>
      <p v-else-if="catsError" class="state-error">{{ catsError }} <button class="retry" type="button" @click="loadCategories">重试</button></p>
      <template v-else>
        <!-- 书单页顶部：书单内搜索（GAP 49 补——前端过滤当前列表，不发请求） -->
        <div class="list-search">
          <svg class="search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
            <circle cx="11" cy="11" r="6.5" />
            <path d="M20 20l-3.8-3.8" />
          </svg>
          <input
            v-model="listFilter"
            class="search-input"
            type="text"
            placeholder="在书单中搜索（书名 / 作者）…"
            spellcheck="false"
          />
          <button v-if="listFilter" class="list-search-clear" type="button" @click="listFilter = ''">清空</button>
        </div>

        <div class="cats">
          <button
            v-for="c in categories"
            :key="c.url"
            type="button"
            class="cat"
            :class="{ active: activeUrl === c.url, link: c.type === 'link' }"
            @click="openCategory(c)"
          >
            <template v-if="c.type === 'link'">↗</template>
            {{ c.title ? applyHan(c.title, hanMode) : '默认' }}
          </button>
        </div>

        <div v-if="loadingBooks" class="state"><p class="state-text">加载中…</p></div>
        <p v-else-if="booksError" class="state-error">{{ booksError }} <button class="retry" type="button" @click="loadBooks(1)">重试</button></p>
        <template v-else>
          <div v-if="books.length === 0" class="state"><p class="state-text">此分类暂无内容</p><p class="state-hint">可能为外部链接/站点失效，或书源探索规则未配置完整</p></div>
          <template v-else>
            <div v-if="filteredBooks.length === 0" class="state">
              <p class="state-text">没有匹配「{{ listFilter.trim() }}」的书籍</p>
            </div>
            <div v-else class="book-grid">
              <button v-for="b in filteredBooks" :key="b.bookUrl" type="button" class="book-card" @click="goBook(b)">
                <span class="book-cover" :style="{ background: coverGradient(b.name) }">
                  <img
                    v-if="b.coverUrl && !failedCovers.has(b.bookUrl)"
                    :src="b.coverUrl"
                    :alt="b.name"
                    loading="lazy"
                    class="cover-img"
                    @error="failedCovers.add(b.bookUrl)"
                  />
                  <template v-else>{{ applyHan(b.name, hanMode).charAt(0) }}</template>
                </span>
                <span class="book-name">{{ applyHan(b.name, hanMode) }}</span>
                <span class="book-author">{{ applyHan(b.author || '未知作者', hanMode) }}</span>
              </button>
            </div>
            <!-- GAP #51：分页（page 参数 + {books, hasMore} 契约；未就绪则标注并隐藏加载更多） -->
            <div class="more-row">
              <p v-if="pagingBackendReady === false" class="paging-note">后端分页接口待实现：当前仅展示第一页</p>
              <template v-else-if="!noMore">
                <button class="more-btn" type="button" :disabled="loadingMore" @click="loadBooks(page + 1)">
                  {{ loadingMore ? '加载中…' : '加载更多' }}
                </button>
                <p class="paging-note">滚动到底部自动加载下一页</p>
              </template>
              <span v-else class="no-more">没有更多了</span>
            </div>
            <!-- 滚动哨兵：进入视口 → 自动加载下一页 -->
            <div v-if="pagingBackendReady === true && !noMore && !loadingBooks" ref="sentinelEl" class="sentinel" aria-hidden="true"></div>
          </template>
        </template>
      </template>
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { applyHan, getHanMode, type HanMode } from '@/utils/chinese'
import { useRouter } from 'vue-router'
import { getExploreSources, getExploreUrls, exploreBook } from '@/api/explore'
import { clearSearchHistory, loadSearchHistory, pushSearchHistory } from '@/utils/searchHistory'
import type { BookSource, ExploreCategory, ExploreSourceInfo, SearchBook } from '@/types'

const router = useRouter()

/* ================= 搜索框（下拉提示最近搜索，与搜索页共用 localStorage 历史） ================= */
const searchKey = ref('')
const showDropdown = ref(false)
const searchHistory = ref<string[]>([])

/** 下拉建议：输入为空 → 全部最近搜索；有输入 → 子串过滤 */
const suggestions = computed(() => {
  const kw = searchKey.value.trim()
  if (!kw) return searchHistory.value
  return searchHistory.value.filter((h) => h.includes(kw))
})

function onSearchEnter() {
  const kw = searchKey.value.trim()
  if (!kw) return
  showDropdown.value = false
  searchHistory.value = pushSearchHistory(kw)
  void router.push({ path: '/search', query: { key: kw } })
}

function clearHistory() {
  clearSearchHistory()
  searchHistory.value = []
}

const sources = ref<ExploreSourceInfo[]>([])
const hanMode = ref<HanMode>(getHanMode())
function toggleHan() {
  hanMode.value = hanMode.value === 'auto' ? 'simp' : hanMode.value === 'simp' ? 'trad' : 'auto'
  setHanModeLocal(hanMode.value)
}
function setHanModeLocal(m: HanMode) {
  try { localStorage.setItem('reader_han_mode', m) } catch { /* ignore */ }
}
const hanLabel = ref('自动')
function updateHanLabel() {
  hanLabel.value = hanMode.value === 'auto' ? '自动' : hanMode.value === 'trad' ? '繁' : '简'
}
updateHanLabel()
const contentWidth = ref('900px')
{
  const raw = localStorage.getItem('reader_content_width')
  if (raw === '720px' || raw === '900px' || raw === '1080px') contentWidth.value = raw
}
const WIDTH_OPTIONS = [
  { label: '窄', value: '720px' },
  { label: '适中', value: '900px' },
  { label: '宽', value: '1080px' },
]
function cycleWidth() {
  const idx = WIDTH_OPTIONS.findIndex((o) => o.value === contentWidth.value)
  contentWidth.value = WIDTH_OPTIONS[(idx + 1) % WIDTH_OPTIONS.length].value
  localStorage.setItem('reader_content_width', contentWidth.value)
}
const sourcesLoading = ref(true)
const sourcesError = ref('')

const source = ref<BookSource | null>(null)
const categories = ref<ExploreCategory[]>([])
const catsLoading = ref(false)
const catsError = ref('')
const activeUrl = ref('')
const books = ref<SearchBook[]>([])
const failedCovers = new Set<string>()
const page = ref(1)
const loadingBooks = ref(false)
const loadingMore = ref(false)
const noMore = ref(false)
const booksError = ref('')

/* ============ 书单内搜索（GAP 49 补：前端过滤当前列表） ============ */
const listFilter = ref('')
const filteredBooks = computed(() => {
  const kw = listFilter.value.trim().toLowerCase()
  if (!kw) return books.value
  return books.value.filter((b) => {
    const name = (b.name ?? '').toLowerCase()
    const author = (b.author ?? '').toLowerCase()
    if (name.includes(kw) || author.includes(kw)) return true
    // 简繁再比一轮（繁体书名 + 简体搜索词等场景）
    const hanName = applyHan(b.name ?? '', hanMode.value).toLowerCase()
    const hanAuthor = applyHan(b.author ?? '', hanMode.value).toLowerCase()
    return hanName.includes(kw) || hanAuthor.includes(kw)
  })
})

/* ============ 分页（GAP #51：后端契约 {books, hasMore}） ============ */
/** 后端分页契约就绪状态：null=未知 / true={books,hasMore} / false=仍返回旧数组 */
const pagingBackendReady = ref<boolean | null>(null)

/** 滚动哨兵：进入视口自动加载下一页（根观察，rootMargin 提前 160px 预载） */
const sentinelEl = ref<HTMLElement | null>(null)
let sentinelObserver: IntersectionObserver | null = null

function tryLoadMore() {
  if (!source.value || !activeUrl.value) return
  if (loadingBooks.value || loadingMore.value || noMore.value) return
  if (pagingBackendReady.value !== true) return
  void loadBooks(page.value + 1)
}

function setupSentinel() {
  sentinelObserver?.disconnect()
  sentinelObserver = null
  const el = sentinelEl.value
  if (!el) return
  sentinelObserver = new IntersectionObserver(
    (entries) => {
      if (entries.some((e) => e.isIntersecting)) tryLoadMore()
    },
    { rootMargin: '160px 0px' },
  )
  sentinelObserver.observe(el)
}

watch(sentinelEl, (el) => {
  if (el) setupSentinel()
  else {
    sentinelObserver?.disconnect()
    sentinelObserver = null
  }
})

const GRADIENTS = [
  'linear-gradient(150deg, #6366f1 0%, #a855f7 100%)',
  'linear-gradient(150deg, #0ea5e9 0%, #22d3ee 100%)',
  'linear-gradient(150deg, #f43f5e 0%, #fb923c 100%)',
  'linear-gradient(150deg, #10b981 0%, #34d399 100%)',
  'linear-gradient(150deg, #8b5cf6 0%, #ec4899 100%)',
  'linear-gradient(150deg, #f59e0b 0%, #ef4444 100%)',
]
function coverGradient(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return GRADIENTS[h % GRADIENTS.length]
}

function exploreCount(s: ExploreSourceInfo): number {
  return s.categoryCount ?? 0
}

async function loadSources() {
  sourcesLoading.value = true
  sourcesError.value = ''
  try {
    const res = await getExploreSources()
    sources.value = (res.data ?? []) as ExploreSourceInfo[]
  } catch {
    sourcesError.value = '书源加载失败'
  } finally {
    sourcesLoading.value = false
  }
}

function selectSource(s: ExploreSourceInfo) {
  const full = sources.value.find((x) => x.bookSourceUrl === s.bookSourceUrl)
  source.value = {
    bookSourceUrl: s.bookSourceUrl,
    bookSourceName: s.bookSourceName,
    enabledExplore: true,
    exploreUrl: '',
  } as unknown as BookSource
  void full
  categories.value = []
  activeUrl.value = ''
  books.value = []
  page.value = 1
  noMore.value = false
  loadCategories()
}

function backToSources() {
  source.value = null
}

async function loadCategories() {
  if (!source.value) return
  catsLoading.value = true
  catsError.value = ''
  try {
    const res = await getExploreUrls(source.value.bookSourceUrl)
    // 后端返回 [{title, url}]（JS 已执行）
    const entries = (res.data ?? []) as ExploreCategory[]
    categories.value = entries
    if (entries.length > 0) {
      activeUrl.value = entries[0].url
      await loadBooks(1)
    }
  } catch {
    catsError.value = '分类加载失败'
  } finally {
    catsLoading.value = false
  }
}

function switchCategory(url: string) {
  if (activeUrl.value === url) return
  activeUrl.value = url
  books.value = []
  noMore.value = false
  loadBooks(1)
}

function openCategory(c: ExploreCategory) {
  if (c.type === 'link') {
    window.open(c.url, '_blank', 'noopener')
    return
  }
  switchCategory(c.url)
}

/** GAP #51 契约：{books, hasMore}（后端并行实现中；未就绪时仍返回旧数组） */
interface ExploreBookPage {
  books: SearchBook[]
  hasMore: boolean
}

async function loadBooks(p: number) {
  if (!source.value || !activeUrl.value) return
  if (p === 1) loadingBooks.value = true
  else loadingMore.value = true
  booksError.value = ''
  try {
    const res = await exploreBook(activeUrl.value, source.value.bookSourceUrl, p)
    const data = res.data as unknown
    let list: SearchBook[] = []
    let hasMore = false
    if (data && typeof data === 'object' && !Array.isArray(data) && Array.isArray((data as ExploreBookPage).books)) {
      // 新契约：后端分页就绪
      const pageData = data as ExploreBookPage
      list = pageData.books
      hasMore = !!pageData.hasMore
      pagingBackendReady.value = true
    } else if (Array.isArray(data)) {
      // 旧契约：后端分页未就绪——仅展示第一页，标注待实现
      list = data as SearchBook[]
      hasMore = false
      pagingBackendReady.value = false
    }
    if (p === 1) books.value = list
    else {
      const seen = new Set(books.value.map((b) => b.bookUrl))
      books.value.push(...list.filter((b) => !seen.has(b.bookUrl)))
    }
    page.value = p
    noMore.value = !hasMore
  } catch {
    if (p === 1) booksError.value = '探索失败'
  } finally {
    loadingBooks.value = false
    loadingMore.value = false
    // 首屏未填满时哨兵仍在视口内（IO 不会重复触发）——补一次可见性检查
    void nextTick(() => {
      const el = sentinelEl.value
      if (!el) return
      const rect = el.getBoundingClientRect()
      if (rect.top < window.innerHeight + 160) tryLoadMore()
    })
  }
}

function goBook(b: SearchBook) {
  router.push(`/book/${encodeURIComponent(b.bookUrl)}`)
}

function onCatsWheel(e: WheelEvent) {
  const el = e.currentTarget as HTMLElement
  if (el.scrollWidth > el.clientWidth) {
    e.preventDefault()
    el.scrollLeft += e.deltaY
  }
}

onMounted(() => {
  loadSources()
  searchHistory.value = loadSearchHistory()
  const catsEl = document.querySelector('.cats')
  catsEl?.addEventListener('wheel', onCatsWheel as EventListener, { passive: false })
})
onBeforeUnmount(() => {
  sentinelObserver?.disconnect()
  sentinelObserver = null
  document.querySelector('.cats')?.removeEventListener('wheel', onCatsWheel as EventListener)
})
</script>

<style scoped>
.page {
  min-height: 100vh;
  background: var(--bg, #fafafa);
}
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 20px;
  background: var(--bg-float);
  backdrop-filter: blur(8px);
  border-bottom: 1px solid var(--border, #ececec);
}
.back-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  padding: 0;
  background: none;
  border: none;
  color: var(--text-1, #1a1a1a);
  cursor: pointer;
}
.back-btn svg {
  width: 20px;
  height: 20px;
}
.brand {
  font-size: 16px;
  font-weight: 300;
  letter-spacing: 1px;
}
.brand em {
  font-style: normal;
  color: var(--accent, #4f46e5);
}
.title {
  flex: 1;
  font-size: 14px;
  font-weight: 300;
  color: var(--text-2, #666);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.top-actions {
  display: flex;
  gap: 6px;
}
.han-btn {
  padding: 4px 12px;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-2, #666);
  background: none;
  border: 1px solid var(--border, #ececec);
  border-radius: 999px;
  cursor: pointer;
  transition: all 0.2s ease;
}
.han-btn:hover {
  border-color: var(--accent, #4f46e5);
  color: var(--accent, #4f46e5);
}
.main {
  margin: 0 auto;
  padding: 24px 20px 80px;
}
.page-hint {
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3, #999);
  letter-spacing: 2px;
  margin: 0 0 16px;
}

/* ================= 搜索框 + 下拉提示 ================= */
.search-wrap {
  position: relative;
  margin-bottom: 20px;
}
.explore-search {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 2px 8px;
  border-bottom: 1px solid var(--border, #ececec);
  transition: border-color 0.2s ease;
}
.explore-search:focus-within {
  border-bottom-color: var(--accent, #4f46e5);
}
.search-icon {
  width: 15px;
  height: 15px;
  flex-shrink: 0;
  color: var(--text-3, #999);
  transition: color 0.2s ease;
}
.explore-search:focus-within .search-icon {
  color: var(--accent, #4f46e5);
}
.search-input {
  flex: 1;
  min-width: 0;
  border: none;
  background: none;
  color: var(--text-1, #1a1a1a);
  font-family: inherit;
  font-size: 15px;
  font-weight: 300;
  letter-spacing: 1px;
  outline: none;
}
.search-input::placeholder {
  color: var(--text-3, #999);
  font-weight: 300;
}
.search-go {
  flex-shrink: 0;
  padding: 4px 14px;
  font-size: 12px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--accent, #4f46e5);
  background: none;
  border: 1px solid var(--accent, #4f46e5);
  border-radius: 999px;
  cursor: pointer;
  transition: all 0.2s ease;
}
.search-go:hover:not(:disabled) {
  background: var(--accent-soft, #eef2ff);
}
.search-go:disabled {
  opacity: 0.4;
  cursor: default;
}
.search-dropdown {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  z-index: 30;
  padding: 8px 4px;
  background: var(--surface, #fff);
  border: 1px solid var(--border, #ececec);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.08);
}
.dropdown-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 2px 10px 6px;
}
.dropdown-label {
  font-size: 11.5px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-3, #999);
}
.dropdown-clear {
  border: none;
  background: none;
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3, #999);
  cursor: pointer;
  transition: color 0.2s ease;
}
.dropdown-clear:hover {
  color: var(--accent, #4f46e5);
}
.dropdown-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 260px;
  overflow-y: auto;
}
.dropdown-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 300;
  color: var(--text-2, #666);
  cursor: pointer;
  transition: background-color 0.15s ease;
}
.dropdown-item:hover {
  background: var(--hover, #f5f5f5);
}
.item-icon {
  flex-shrink: 0;
  width: 11px;
  height: 11px;
  color: var(--text-3, #999);
}
.item-text {
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.state {
  padding: 60px 0;
  text-align: center;
}
.state-text {
  font-size: 13px;
  font-weight: 300;
  color: var(--text-3, #999);
}
.state-hint {
  margin-top: 8px;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3, #999);
  opacity: 0.7;
}
.state-error {
  padding: 40px 0;
  text-align: center;
  font-size: 13px;
  color: #e11d48;
}
.retry {
  margin-left: 8px;
  padding: 4px 12px;
  font-size: 12px;
  color: var(--accent, #4f46e5);
  background: none;
  border: 1px solid var(--accent, #4f46e5);
  border-radius: 999px;
  cursor: pointer;
}
.link {
  display: inline-block;
  margin-top: 12px;
  font-size: 13px;
  color: var(--accent, #4f46e5);
  text-decoration: none;
}
.source-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.source-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  background: var(--surface, #fff);
  border: 1px solid var(--border, #ececec);
  border-radius: 8px;
  cursor: pointer;
  transition: border-color 0.2s ease, transform 0.2s ease;
}
.source-item:hover {
  border-color: var(--accent, #4f46e5);
  transform: translateY(-1px);
}
.source-name {
  flex: 1;
  font-size: 14px;
  font-weight: 400;
  color: var(--text-1, #1a1a1a);
}
.source-count {
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3, #999);
}
.chevron {
  color: var(--text-3, #999);
  font-size: 16px;
}
.cats {
  display: flex;
  gap: 6px;
  margin-bottom: 20px;
  padding-bottom: 4px;
  overflow-x: auto;
  scrollbar-width: none;
  -webkit-overflow-scrolling: touch;
  touch-action: pan-x;
  overscroll-behavior-x: contain;
  cursor: grab;
}
.cats:active {
  cursor: grabbing;
}
.cats::-webkit-scrollbar {
  display: none;
}
.cat {
  flex-shrink: 0;
  padding: 6px 14px;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-2, #666);
  background: none;
  border: 1px solid var(--border, #ececec);
  border-radius: 999px;
  cursor: pointer;
  transition: all 0.2s ease;
}
.cat:hover {
  border-color: var(--accent, #4f46e5);
  color: var(--accent, #4f46e5);
}
.cat.active {
  border-color: var(--accent, #4f46e5);
  color: var(--accent, #4f46e5);
  background: var(--accent-soft, #eef2ff);
}
.cat.link {
  border-style: dashed;
  color: var(--text-3, #999);
}
.book-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  gap: 20px 16px;
}
.book-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 0;
  background: none;
  border: none;
  text-align: left;
  cursor: pointer;
}
.book-cover {
  aspect-ratio: 3 / 4;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 32px;
  font-weight: 300;
  color: rgba(255, 255, 255, 0.9);
  transition: transform 0.2s ease;
  overflow: hidden;
}
.cover-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.book-card:hover .book-cover {
  transform: translateY(-2px);
}
.book-name {
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1, #1a1a1a);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-author {
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3, #999);
}
.more-row {
  margin-top: 24px;
  text-align: center;
}
.more-btn {
  padding: 8px 24px;
  font-size: 13px;
  font-weight: 400;
  color: var(--accent, #4f46e5);
  background: none;
  border: 1px solid var(--accent, #4f46e5);
  border-radius: 999px;
  cursor: pointer;
  transition: all 0.2s ease;
}
.more-btn:hover:not(:disabled) {
  background: var(--accent-soft, #eef2ff);
}
.more-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.no-more {
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3, #999);
}

/* ================= 书单内搜索 + 分页 ================= */
.list-search {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 2px 8px;
  margin-bottom: 14px;
  border-bottom: 1px solid var(--border, #ececec);
  transition: border-color 0.2s ease;
}
.list-search:focus-within {
  border-bottom-color: var(--accent, #4f46e5);
}
.list-search:focus-within .search-icon {
  color: var(--accent, #4f46e5);
}
.list-search-clear {
  flex-shrink: 0;
  padding: 2px 10px;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3, #999);
  background: none;
  border: none;
  cursor: pointer;
  transition: color 0.2s ease;
}
.list-search-clear:hover {
  color: var(--accent, #4f46e5);
}
.paging-note {
  margin: 8px 0 0;
  font-size: 11.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3, #999);
}
.sentinel {
  height: 1px;
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    padding: 10px 12px;
    gap: 8px;
  }
  .main {
    padding: 20px 14px 64px;
  }
  .cats {
    margin-bottom: 14px;
  }
  .book-grid {
    gap: 16px 12px;
  }
}
@media (max-width: 480px) {
  .book-grid {
    grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
    gap: 14px 10px;
  }
  .book-cover {
    font-size: 26px;
  }
}
</style>
