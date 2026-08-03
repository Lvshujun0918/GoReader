<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { searchBookMulti, searchBookMultiSSE } from '@/api/search'
import { useUserStore } from '@/stores/user'
import { clearSearchHistory, loadSearchHistory, pushSearchHistory } from '@/utils/searchHistory'
import type { ReturnData, SearchBook } from '@/types'

const router = useRouter()
const route = useRoute()

/* ================= 搜索 ================= */
const key = ref('')
const searching = ref(false)
const searched = ref(false)
const errorMsg = ref('')
const stopped = ref(false)
/** 当前搜索是否走 SSE（决定实时源计数是否显示） */
const usingSSE = ref(false)

/** 合并后的结果（bookUrl 去重，同书多源合并来源标签） */
interface MergedResult {
  book: SearchBook
  /** 来源标签（按 origin 去重，显示 originName || origin） */
  origins: { key: string; label: string }[]
}

const results = ref<MergedResult[]>([])
const failedCovers = new Set<string>()
/** 已返回结果的书源数（SSE 每源一个 book 事件，lastIndex 去重计数） */
const searchedSources = ref(0)

const bookMap = new Map<string, MergedResult>()
const completedSources = new Set<number>()
let sseAbort: (() => void) | null = null
/** 搜索代数：取消/停止后使在途 SSE/批量响应失效 */
let searchSeq = 0

function labelOf(b: SearchBook): string {
  return b.originName || b.origin
}

/** bookUrl 去重合并：新书入表；同书追加来源标签并补全缺失展示字段 */
function mergeBooks(books: SearchBook[]) {
  for (const b of books) {
    let entry = bookMap.get(b.bookUrl)
    if (!entry) {
      entry = { book: b, origins: [] }
      bookMap.set(b.bookUrl, entry)
    }
    const okey = b.origin || labelOf(b)
    if (!entry.origins.some((o) => o.key === okey)) {
      entry.origins.push({ key: okey, label: labelOf(b) })
    }
    const cur = entry.book
    if (!cur.intro && b.intro) cur.intro = b.intro
    if (!cur.latestChapterTitle && b.latestChapterTitle) cur.latestChapterTitle = b.latestChapterTitle
    if (!cur.wordCount && b.wordCount) cur.wordCount = b.wordCount
    if (!cur.author && b.author) cur.author = b.author
  }
  results.value = Array.from(bookMap.values())
}

/** 服务端业务错误（event: error）：NEED_LOGIN 跳登录，其余展示错误 */
function handleErrorEvent(ret: ReturnData) {
  if (ret.data === 'NEED_LOGIN' || (ret.errorMsg || '').includes('请登录')) {
    const store = useUserStore()
    store.clear()
    void router.replace({ path: '/login', query: { redirect: router.currentRoute.value.fullPath } })
    return
  }
  errorMsg.value = ret.errorMsg || '搜索失败，请稍后重试'
  searching.value = false
  searched.value = true
}

async function doSearch(kw?: string) {
  const word = (kw ?? key.value).trim()
  if (!word || searching.value) return
  key.value = word
  const seq = ++searchSeq
  sseAbort = null
  searching.value = true
  searched.value = false
  errorMsg.value = ''
  stopped.value = false
  usingSSE.value = true
  results.value = []
  bookMap.clear()
  completedSources.clear()
  searchedSources.value = 0

  // 1) 优先 SSE 流式搜索（增量显示）
  try {
    const handle = await searchBookMultiSSE(
      {
        key: word,
        bookSourceGroup: '',
        lastIndex: -1,
        searchSize: 50,
        concurrentCount: 12,
      },
      {
        onBooks: (lastIndex, books) => {
          if (seq !== searchSeq) return
          if (lastIndex >= 0) completedSources.add(lastIndex)
          searchedSources.value = completedSources.size
          mergeBooks(books)
        },
        onEnd: () => {
          if (seq !== searchSeq) return
          searching.value = false
          searched.value = true
          pushHistory(word)
        },
        onErrorEvent: (ret) => {
          if (seq !== searchSeq) return
          handleErrorEvent(ret)
        },
        onStreamError: (msg) => {
          if (seq !== searchSeq) return
          errorMsg.value = msg
          searching.value = false
          searched.value = true
        },
      },
    )
    if (seq !== searchSeq) {
      // 等待连接期间已被停止
      handle.abort()
      return
    }
    sseAbort = handle.abort
  } catch {
    // 2) SSE 传输失败/不支持 → 降级批量模式
    if (seq !== searchSeq) return
    usingSSE.value = false
    await runBatch(word, seq)
  }
}

/** 批量降级：现有 searchBookMulti（maxSources=50），一次性出结果 */
async function runBatch(word: string, seq: number) {
  try {
    const res = await searchBookMulti(word, 50)
    if (seq !== searchSeq) return
    if (!res.isSuccess) {
      if ((res.data as unknown) === 'NEED_LOGIN' || (res.errorMsg || '').includes('请登录')) {
        const store = useUserStore()
        store.clear()
        void router.replace({ path: '/login', query: { redirect: router.currentRoute.value.fullPath } })
        return
      }
      throw new Error(res.errorMsg || '搜索失败，请稍后重试')
    }
    mergeBooks(res.data ?? [])
    searched.value = true
    pushHistory(word)
  } catch (err) {
    if (seq !== searchSeq) return
    errorMsg.value = err instanceof Error ? err.message : '搜索失败，请稍后重试'
  } finally {
    if (seq === searchSeq) searching.value = false
  }
}

/** 停止搜索：中断 SSE，保留已到达的部分结果 */
function stopSearch() {
  if (!searching.value) return
  searchSeq++
  if (sseAbort) {
    sseAbort()
    sseAbort = null
  }
  stopped.value = true
  searched.value = true
  searching.value = false
  pushHistory(key.value.trim())
}

function onEnter() {
  void doSearch()
}

function openBook(book: SearchBook) {
  void router.push(`/book/${encodeURIComponent(book.bookUrl)}`)
}

/* ================= 搜索历史（localStorage，最近 10 条——与探索页共用） ================= */
const history = ref<string[]>([])

function loadHistory() {
  history.value = loadSearchHistory()
}

function pushHistory(word: string) {
  history.value = pushSearchHistory(word)
}

function clearHistory() {
  clearSearchHistory()
  history.value = []
}

onMounted(() => {
  loadHistory()
  // 支持 /search?key=xxx 预填并自动搜索（阅读页划词「搜索」跳转）
  const kw = typeof route.query.key === 'string' ? route.query.key.trim() : ''
  if (kw) void doSearch(kw)
})
</script>

<template>
  <div class="search-page">
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
      <h1 class="page-title">搜索</h1>

      <!-- 极简搜索框：细字 + 下划线 focus 强调色 + 搜索按钮 -->
      <div class="search-bar">
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
          v-model="key"
          class="search-input"
          type="text"
          placeholder="书名 / 作者"
          spellcheck="false"
          @keydown.enter="onEnter"
        />
        <button class="search-btn" type="button" :disabled="searching || !key.trim()" @click="onEnter">
          {{ searching ? '搜索中…' : '搜索' }}
        </button>
      </div>

      <!-- 搜索历史 -->
      <div v-if="history.length && !searching && !searched && !errorMsg" class="history">
        <div class="history-head">
          <span class="history-label">搜索历史</span>
          <button class="history-clear" type="button" @click="clearHistory">清空</button>
        </div>
        <div class="history-chips">
          <button
            v-for="h in history"
            :key="h"
            class="history-chip"
            type="button"
            @click="doSearch(h)"
          >
            {{ h }}
          </button>
        </div>
      </div>

      <!-- 加载态：实时源计数（SSE）+ 停止按钮 -->
      <div v-if="searching" class="state-row" aria-live="polite">
        <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
          <path d="M21 12a9 9 0 1 1-6.2-8.56" />
        </svg>
        <span class="state-text">
          {{ usingSSE ? `正在搜索 · 已搜索 ${searchedSources} 个书源` : '正在搜索多个书源…' }}
        </span>
        <button class="stop-btn" type="button" @click="stopSearch">停止</button>
      </div>

      <!-- 错误态（无结果时整行展示） -->
      <div v-else-if="errorMsg && !results.length" class="state-row">
        <span class="state-text error">{{ errorMsg }}</span>
        <button class="retry-btn" type="button" @click="doSearch()">重试</button>
      </div>

      <!-- 空结果 / 已停止 -->
      <div v-else-if="searched && !results.length" class="state-row">
        <span class="state-text">{{ stopped ? '已停止搜索' : `没有找到与「${key.trim()}」相关的书籍` }}</span>
      </div>

      <!-- 结果列表（SSE 增量累积） -->
      <div v-if="results.length" class="results-wrap">
        <p v-if="errorMsg" class="result-note error">{{ errorMsg }}</p>
        <p v-else-if="stopped" class="result-note">已停止 · 以下为部分结果</p>
        <p class="result-meta">
          共 {{ results.length }} 本书<span v-if="searchedSources"> · 来自 {{ searchedSources }} 个书源</span>
        </p>
        <ul class="result-list">
          <li
            v-for="entry in results"
            :key="entry.book.bookUrl"
            class="result-item"
            @click="openBook(entry.book)"
          >
            <span v-if="entry.book.coverUrl && !failedCovers.has(entry.book.bookUrl)" class="result-cover">
              <img :src="entry.book.coverUrl" :alt="entry.book.name" loading="lazy" @error="failedCovers.add(entry.book.bookUrl)" />
            </span>
            <span v-else class="result-cover placeholder">{{ entry.book.name.charAt(0) }}</span>
            <div class="result-main">
              <p class="result-name" :title="entry.book.name">{{ entry.book.name }}</p>
              <p class="result-sub">
                <span class="result-author">{{ entry.book.author || '佚名' }}</span>
                <span
                  v-for="o in entry.origins"
                  :key="o.key"
                  class="source-badge"
                  :title="o.label"
                >{{ o.label }}</span>
                <span v-if="entry.book.latestChapterTitle" class="result-chapter" :title="entry.book.latestChapterTitle">
                  {{ entry.book.latestChapterTitle }}
                </span>
              </p>
              <p v-if="entry.book.intro" class="result-intro">{{ entry.book.intro }}</p>
            </div>
            <svg class="result-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
              <path d="M9 6l6 6-6 6" />
            </svg>
          </li>
        </ul>
      </div>
    </main>
  </div>
</template>

<style scoped>
.search-page {
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
  background: var(--bg-float);
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
  width: min(720px, 100%);
  margin: 0 auto;
  padding: 44px 32px 72px;
}
.page-title {
  margin: 0 0 26px;
  font-size: 22px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-1);
}

/* 搜索框：细字 + 下划线 focus 强调色 */
.search-bar {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 4px 0 10px;
  border-bottom: 1px solid var(--border);
  transition: border-color 0.2s ease;
}
.search-bar:focus-within {
  border-bottom-color: var(--accent);
}
.search-icon {
  width: 17px;
  height: 17px;
  flex-shrink: 0;
  color: var(--text-3);
  transition: color 0.2s ease;
}
.search-bar:focus-within .search-icon {
  color: var(--accent);
}
.search-input {
  flex: 1;
  min-width: 0;
  border: none;
  background: none;
  color: var(--text-1);
  font-family: inherit;
  font-size: 19px;
  font-weight: 300;
  letter-spacing: 1px;
  outline: none;
}
.search-input::placeholder {
  color: var(--text-3);
  font-weight: 300;
}
.search-btn {
  flex-shrink: 0;
  padding: 6px 18px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: none;
  color: var(--accent);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 2px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.search-btn:hover:not(:disabled) {
  color: var(--accent-deep);
  border-color: var(--accent-deep);
  background: var(--accent-soft);
}
.search-btn:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}

/* ================= 搜索历史 ================= */
.history {
  margin-top: 22px;
}
.history-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}
.history-label {
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-3);
}
.history-clear {
  border: none;
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  cursor: pointer;
  transition: color 0.2s ease;
}
.history-clear:hover {
  color: var(--accent);
}
.history-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.history-chip {
  padding: 4px 12px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.history-chip:hover {
  color: var(--accent);
  border-color: var(--accent);
}

/* ================= 状态行（加载 / 空 / 错误） ================= */
.state-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 72px 0;
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
.stop-btn {
  padding: 5px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: none;
  color: var(--accent);
  font-family: inherit;
  font-size: 12px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.stop-btn:hover {
  color: var(--accent-deep);
  border-color: var(--accent-deep);
  background: var(--accent-soft);
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

/* ================= 结果列表 ================= */
.results-wrap {
  margin-top: 22px;
}
.result-meta {
  margin: 0;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.result-note {
  margin: 0 0 10px;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.result-note.error {
  color: #cf4444;
}
.result-list {
  list-style: none;
  margin: 10px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}
.result-item {
  display: flex;
  align-items: center;
  gap: 12px;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 16px 4px;
  border-bottom: 1px solid var(--border);
  cursor: pointer;
  transition: background-color 0.2s ease;
}
.result-item:first-child {
  border-top: 1px solid var(--border);
}
.result-item:hover {
  background: var(--surface);
}
.result-cover {
  flex-shrink: 0;
  width: 44px;
  height: 58px;
  border-radius: 6px;
  overflow: hidden;
  background: var(--accent-soft, #eef2ff);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 18px;
  font-weight: 300;
  color: var(--accent, #4f46e5);
}
.result-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.result-cover.placeholder {
  font-size: 18px;
  color: var(--text-3, #999);
  background: var(--border, #ececec);
}
.result-main {
  flex: 1;
  min-width: 0;
}
.result-name {
  margin: 0;
  font-size: 14.5px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.result-sub {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px 10px;
  margin: 5px 0 0;
  min-width: 0;
}
.result-author {
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
  flex-shrink: 0;
}
/* 来源徽标：细字描边（同书多源时展示多枚） */
.source-badge {
  flex-shrink: 0;
  max-width: 140px;
  padding: 1px 8px;
  border-radius: 4px;
  border: 1px solid var(--border-strong);
  color: var(--text-2);
  font-size: 11px;
  font-weight: 300;
  letter-spacing: 0.5px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.result-chapter {
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.result-intro {
  margin: 7px 0 0;
  font-size: 12.5px;
  font-weight: 300;
  line-height: 1.6;
  color: var(--text-2);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.result-arrow {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  color: var(--text-3);
  transition: color 0.2s ease, transform 0.2s ease;
}
.result-item:hover .result-arrow {
  color: var(--accent);
  transform: translateX(2px);
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    padding: 12px 16px;
  }
  .content {
    padding: 32px 16px 56px;
  }
  .search-input {
    font-size: 17px;
  }
  .search-btn {
    padding: 6px 14px;
  }
}
</style>
