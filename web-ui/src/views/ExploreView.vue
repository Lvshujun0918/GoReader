<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { getExploreUrls, exploreBook } from '@/api/explore'
import { getBookSources } from '@/api/sources'
import type { BookSource, ExploreCategory, SearchBook } from '@/types'

const router = useRouter()

/* ================= 书源（简化：取第一个启用探索的书源） ================= */
const source = ref<BookSource | null>(null)
const sourceCount = ref(0) // 有 exploreUrl 的书源总数（多于 1 时提示当前取第一个）

/* ================= 分类列表 ================= */
const categories = ref<ExploreCategory[]>([])
const catsLoading = ref(true)
const catsError = ref('')

/* ================= 书籍列表 ================= */
const activeUrl = ref('')
const books = ref<SearchBook[]>([])
const page = ref(1)
const loadingBooks = ref(false)
const loadingMore = ref(false)
const noMore = ref(false)
const booksError = ref('')

/** 从 URL 尾部路径或参数提取分类名称 */
function categoryName(url: string): string {
  try {
    const u = new URL(url)
    const segs = u.pathname.split('/').filter(Boolean)
    let name = segs.length ? decodeURIComponent(segs[segs.length - 1]) : ''
    if (!name) {
      name =
        u.searchParams.get('name') ??
        u.searchParams.get('type') ??
        u.searchParams.get('id') ??
        ''
    }
    if (!name) name = u.hostname.replace(/^www\./, '')
    return name
  } catch {
    // 非标准 URL：取最后一个 / 后的片段（去 query/hash）
    return url.split(/[?#]/)[0].split('/').filter(Boolean).pop() ?? url
  }
}

/** 加载书源 → 分类 → 自动选中第一个分类 */
async function init() {
  catsLoading.value = true
  catsError.value = ''
  try {
    const res = await getBookSources()
    const candidates = (res.data ?? []).filter((s) => s.enabledExplore && s.exploreUrl)
    sourceCount.value = candidates.length
    const chosen = candidates[0] ?? null
    source.value = chosen
    if (!chosen) return // 空态：提示去书源管理

    const urls = await getExploreUrls(chosen.bookSourceUrl)
    const seen = new Set<string>()
    categories.value = (urls.data ?? [])
      .filter((u) => {
        if (seen.has(u)) return false
        seen.add(u)
        return true
      })
      .map((u) => ({ url: u, name: categoryName(u) }))
    if (categories.value.length) {
      activeUrl.value = categories.value[0].url
      await loadBooks(1)
    }
  } catch (err) {
    catsError.value = err instanceof Error ? err.message : '加载失败，请稍后重试'
  } finally {
    catsLoading.value = false
  }
}

function dedupe(list: SearchBook[]): SearchBook[] {
  const seen = new Set<string>()
  return list.filter((b) => {
    const k = `${b.origin}|${b.bookUrl}`
    if (seen.has(k)) return false
    seen.add(k)
    return true
  })
}

async function selectCategory(url: string) {
  if (url === activeUrl.value || loadingBooks.value) return
  activeUrl.value = url
  books.value = []
  page.value = 1
  noMore.value = false
  booksError.value = ''
  await loadBooks(1)
}

async function loadBooks(p: number) {
  if (!source.value || !activeUrl.value) return
  loadingBooks.value = true
  booksError.value = ''
  try {
    const res = await exploreBook(activeUrl.value, source.value.bookSourceUrl, p)
    books.value = dedupe(res.data ?? [])
    page.value = p
    noMore.value = (res.data ?? []).length === 0
  } catch (err) {
    booksError.value = err instanceof Error ? err.message : '加载失败，请稍后重试'
  } finally {
    loadingBooks.value = false
  }
}

/** 加载更多：page+1 追加，按 bookUrl 去重；返回空页视为到底 */
async function loadMore() {
  if (!source.value || loadingMore.value || noMore.value) return
  loadingMore.value = true
  try {
    const next = page.value + 1
    const res = await exploreBook(activeUrl.value, source.value.bookSourceUrl, next)
    const list = res.data ?? []
    page.value = next
    const seen = new Set(books.value.map((b) => b.bookUrl))
    books.value.push(...list.filter((b) => !seen.has(b.bookUrl)))
    noMore.value = list.length === 0
  } catch {
    // 错误提示已由拦截器统一处理，保留现有列表
  } finally {
    loadingMore.value = false
  }
}

function openBook(book: SearchBook) {
  void router.push(`/book/${encodeURIComponent(book.bookUrl)}`)
}

function goSources() {
  void router.push('/sources')
}

/* ================= 封面占位（与书架一致：莫兰迪纯色块） ================= */
const MORANDI = [
  '#9aa8a0',
  '#a5a0b0',
  '#b0a59a',
  '#9aa8b5',
  '#b0a0a5',
  '#a3a79a',
  '#a89fb0',
  '#b5a89c',
]

function hashName(name: string): number {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return h
}

function coverColor(name: string): string {
  return MORANDI[hashName(name) % MORANDI.length]
}

function coverInitial(name: string): string {
  const ch = name.trim().charAt(0)
  return ch ? ch.toUpperCase() : '书'
}

onMounted(() => {
  void init()
})
</script>

<template>
  <div class="explore-page">
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
      <h1 class="page-title">探索</h1>

      <!-- 书源提示：细字 -->
      <p v-if="source" class="source-hint">
        当前书源：{{ source.bookSourceName }}
        <template v-if="sourceCount > 1">（有 {{ sourceCount }} 个书源支持探索，当前取第一个）</template>
      </p>

      <!-- 分类加载态 -->
      <div v-if="catsLoading" class="state-row" aria-live="polite">
        <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
          <path d="M21 12a9 9 0 1 1-6.2-8.56" />
        </svg>
        <span class="state-text">正在加载分类…</span>
      </div>

      <!-- 分类错误态 -->
      <div v-else-if="catsError" class="state-row">
        <span class="state-text error">{{ catsError }}</span>
        <button class="retry-btn" type="button" @click="init">重试</button>
      </div>

      <!-- 无可用书源 -->
      <div v-else-if="!source" class="state-row">
        <span class="state-text">当前没有启用探索的书源，去书源管理添加或启用吧</span>
        <button class="retry-btn" type="button" @click="goSources">书源管理</button>
      </div>

      <!-- 分类胶囊列表（细字，active 强调色） -->
      <template v-else>
        <div v-if="categories.length" class="cat-list">
          <button
            v-for="cat in categories"
            :key="cat.url"
            type="button"
            class="cat-chip"
            :class="{ active: cat.url === activeUrl }"
            :title="cat.url"
            @click="selectCategory(cat.url)"
          >
            {{ cat.name }}
          </button>
        </div>
        <div v-else class="state-row">
          <span class="state-text">该书源没有可探索的分类</span>
        </div>

        <!-- 书籍列表：加载 / 空 / 错误 / 网格 -->
        <div v-if="loadingBooks" class="book-grid" aria-label="加载中">
          <div v-for="i in 12" :key="i" class="skeleton-card">
            <div class="skeleton-cover"></div>
            <div class="skeleton-line"></div>
            <div class="skeleton-line short"></div>
          </div>
        </div>

        <div v-else-if="booksError && books.length === 0" class="state-row">
          <span class="state-text error">{{ booksError }}</span>
          <button class="retry-btn" type="button" @click="loadBooks(1)">重试</button>
        </div>

        <div v-else-if="books.length === 0" class="state-row">
          <span class="state-text">该分类暂无书籍</span>
        </div>

        <template v-else>
          <div class="book-grid">
            <div
              v-for="book in books"
              :key="`${book.origin}-${book.bookUrl}`"
              class="book-card"
              @click="openBook(book)"
            >
              <div class="cover-wrap">
                <img
                  v-if="book.coverUrl"
                  :src="book.coverUrl"
                  class="cover-img"
                  :alt="book.name"
                  loading="lazy"
                />
                <div v-else class="cover-ph" :style="{ background: coverColor(book.name) }">
                  <span class="cover-ph-char">{{ coverInitial(book.name) }}</span>
                </div>
              </div>
              <div class="book-meta">
                <p class="book-name" :title="book.name">{{ book.name }}</p>
                <p class="book-author">{{ book.author || '佚名' }}</p>
              </div>
            </div>
          </div>

          <!-- 加载更多：page+1 -->
          <div class="more-row">
            <button
              v-if="!noMore"
              class="more-btn"
              type="button"
              :disabled="loadingMore"
              @click="loadMore"
            >
              {{ loadingMore ? '加载中…' : '加载更多' }}
            </button>
            <span v-else class="no-more">没有更多了</span>
          </div>
        </template>
      </template>
    </main>
  </div>
</template>

<style scoped>
.explore-page {
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
  gap: 16px;
  padding: 14px 32px;
  background: rgba(250, 250, 250, 0.86);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}
.back-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.back-btn:hover {
  color: var(--accent);
}
.back-btn svg {
  width: 14px;
  height: 14px;
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

/* ================= 内容区 ================= */
.content {
  width: min(1080px, 100%);
  margin: 0 auto;
  padding: 44px 32px 72px;
}
.page-title {
  margin: 0 0 10px;
  font-size: 22px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-1);
}
.source-hint {
  margin: 0 0 24px;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 0.5px;
  color: var(--text-3);
}

/* ================= 分类胶囊（细字描边，active 强调色） ================= */
.cat-list {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 30px;
  padding-bottom: 20px;
  border-bottom: 1px solid var(--border);
}
.cat-chip {
  max-width: 220px;
  padding: 4px 13px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 0.5px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.cat-chip:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.cat-chip.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
  font-weight: 400;
}

/* ================= 书籍网格（复用书架卡片风格） ================= */
.book-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 30px 26px;
}
.book-card {
  cursor: pointer;
  transition: transform 0.2s ease;
}
.book-card:hover {
  transform: translateY(-4px);
}
.cover-wrap {
  position: relative;
  aspect-ratio: 3 / 4;
  border-radius: 10px;
  overflow: hidden;
  border: 1px solid var(--border);
  background: var(--surface);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  transition: border-color 0.2s ease;
}
.book-card:hover .cover-wrap {
  border-color: var(--accent);
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
}
.cover-ph-char {
  font-size: 40px;
  font-weight: 300;
  color: rgba(255, 255, 255, 0.94);
  letter-spacing: 2px;
}
.book-meta {
  padding: 10px 2px 0;
}
.book-name {
  margin: 0;
  font-size: 13.5px;
  font-weight: 500;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-author {
  margin: 4px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ================= 骨架屏 ================= */
.skeleton-cover {
  aspect-ratio: 3 / 4;
  border-radius: 10px;
  background: #f0f0f2;
  border: 1px solid var(--border);
}
.skeleton-line {
  height: 11px;
  margin-top: 12px;
  border-radius: 4px;
  background: #f0f0f2;
}
.skeleton-line.short {
  width: 55%;
  margin-top: 8px;
}

/* ================= 状态行 ================= */
.state-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 64px 0;
}
.state-text {
  font-size: 13.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.state-text.error {
  color: #cf4444;
}
.mini-spin {
  width: 13px;
  height: 13px;
  color: var(--accent);
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
.retry-btn {
  padding: 5px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--border-strong);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12px;
  font-weight: 400;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.retry-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
}

/* ================= 加载更多 ================= */
.more-row {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 36px 0 8px;
}
.more-btn {
  padding: 7px 26px;
  border-radius: var(--radius);
  border: 1px solid var(--border-strong);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.more-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.more-btn:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.no-more {
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    padding: 12px 16px;
  }
  .content {
    padding: 32px 16px 56px;
  }
  .book-grid {
    grid-template-columns: repeat(auto-fill, minmax(112px, 1fr));
    gap: 22px 14px;
  }
  .cat-chip {
    max-width: 160px;
  }
}
</style>
