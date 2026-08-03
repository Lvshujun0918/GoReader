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
      <span class="title">{{ source ? source.bookSourceName : '探索' }}</span>
      <div class="top-actions"></div>
    </header>

    <!-- 书源列表（legado 语义：所有 enabledExplore 书源） -->
    <main v-if="!source" class="main">
      <p class="page-hint">书源探索</p>
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
          <span class="source-name">{{ s.bookSourceName }}</span>
          <span class="source-count">{{ exploreCount(s) }} 个分类</span>
          <span class="chevron">›</span>
        </li>
      </ul>
    </main>

    <!-- 书源探索页：分类 + 书籍 -->
    <main v-else class="main">
      <div v-if="catsLoading" class="state"><p class="state-text">加载分类…</p></div>
      <p v-else-if="catsError" class="state-error">{{ catsError }} <button class="retry" type="button" @click="loadCategories">重试</button></p>
      <template v-else>
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
            {{ c.title || '默认' }}
          </button>
        </div>

        <div v-if="loadingBooks" class="state"><p class="state-text">加载中…</p></div>
        <p v-else-if="booksError" class="state-error">{{ booksError }} <button class="retry" type="button" @click="loadBooks(1)">重试</button></p>
        <div v-else-if="books.length === 0" class="state"><p class="state-text">此分类暂无内容</p><p class="state-hint">可能为外部链接/站点失效，或书源探索规则未配置完整</p></div>
        <div v-else class="book-grid">
          <button v-for="b in books" :key="b.bookUrl" type="button" class="book-card" @click="goBook(b)">
            <span class="book-cover" :style="{ background: coverGradient(b.name) }">
              <img
                v-if="b.coverUrl && !failedCovers.has(b.bookUrl)"
                :src="b.coverUrl"
                :alt="b.name"
                loading="lazy"
                class="cover-img"
                @error="failedCovers.add(b.bookUrl)"
              />
              <template v-else>{{ b.name.charAt(0) }}</template>
            </span>
            <span class="book-name">{{ b.name }}</span>
            <span class="book-author">{{ b.author || '未知作者' }}</span>
          </button>
        </div>
        <div v-if="books.length > 0" class="more-row">
          <button v-if="!noMore" class="more-btn" type="button" :disabled="loadingMore" @click="loadBooks(page + 1)">
            {{ loadingMore ? '加载中…' : '加载更多' }}
          </button>
          <span v-else class="no-more">没有更多了</span>
        </div>
      </template>
    </main>
  </div>
</template>

<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { getExploreSources, getExploreUrls, exploreBook } from '@/api/explore'
import type { BookSource, ExploreCategory, ExploreSourceInfo, SearchBook } from '@/types'

const router = useRouter()

const sources = ref<ExploreSourceInfo[]>([])
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

async function loadBooks(p: number) {
  if (!source.value || !activeUrl.value) return
  if (p === 1) loadingBooks.value = true
  else loadingMore.value = true
  booksError.value = ''
  try {
    const res = await exploreBook(activeUrl.value, source.value.bookSourceUrl, p)
    const list = (res.data ?? []) as SearchBook[]
    if (p === 1) books.value = list
    else {
      const seen = new Set(books.value.map((b) => b.bookUrl))
      books.value.push(...list.filter((b) => !seen.has(b.bookUrl)))
    }
    page.value = p
    noMore.value = list.length === 0
  } catch {
    if (p === 1) booksError.value = '探索失败'
  } finally {
    loadingBooks.value = false
    loadingMore.value = false
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
  const catsEl = document.querySelector('.cats')
  catsEl?.addEventListener('wheel', onCatsWheel as EventListener, { passive: false })
})
onBeforeUnmount(() => {
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
  background: rgba(250, 250, 250, 0.86);
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
  width: 30px;
}
.main {
  max-width: 720px;
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
</style>
