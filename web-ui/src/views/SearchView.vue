<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { searchBookMulti } from '@/api/search'
import type { SearchBook } from '@/types'

const router = useRouter()

/* ================= 搜索 ================= */
const key = ref('')
const searching = ref(false)
const searched = ref(false)
const errorMsg = ref('')
const results = ref<SearchBook[]>([])

async function doSearch(kw?: string) {
  const word = (kw ?? key.value).trim()
  if (!word || searching.value) return
  key.value = word
  searching.value = true
  searched.value = false
  errorMsg.value = ''
  results.value = []
  try {
    const res = await searchBookMulti(word, 50)
    results.value = res.data ?? []
    searched.value = true
    pushHistory(word)
  } catch (err) {
    errorMsg.value = err instanceof Error ? err.message : '搜索失败，请稍后重试'
  } finally {
    searching.value = false
  }
}

function onEnter() {
  void doSearch()
}

function openBook(book: SearchBook) {
  void router.push(`/book/${encodeURIComponent(book.bookUrl)}`)
}

/* ================= 搜索历史（localStorage，最近 10 条） ================= */
const HISTORY_KEY = 'reader_search_history'
const history = ref<string[]>([])

function loadHistory() {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    history.value = raw ? (JSON.parse(raw) as string[]) : []
  } catch {
    history.value = []
  }
}

function pushHistory(word: string) {
  const next = [word, ...history.value.filter((h) => h !== word)].slice(0, 10)
  history.value = next
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(next))
  } catch {
    // localStorage 不可用时静默降级
  }
}

function clearHistory() {
  history.value = []
  try {
    localStorage.removeItem(HISTORY_KEY)
  } catch {
    // 忽略
  }
}

onMounted(loadHistory)
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

      <!-- 加载态：细字 + 微 spinner -->
      <div v-if="searching" class="state-row" aria-live="polite">
        <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
          <path d="M21 12a9 9 0 1 1-6.2-8.56" />
        </svg>
        <span class="state-text">正在搜索多个书源…</span>
      </div>

      <!-- 错误态 -->
      <div v-else-if="errorMsg" class="state-row">
        <span class="state-text error">{{ errorMsg }}</span>
        <button class="retry-btn" type="button" @click="doSearch()">重试</button>
      </div>

      <!-- 空结果 -->
      <div v-else-if="searched && results.length === 0" class="state-row">
        <span class="state-text">没有找到与「{{ key.trim() }}」相关的书籍</span>
      </div>

      <!-- 结果列表 -->
      <ul v-else-if="results.length" class="result-list">
        <li
          v-for="book in results"
          :key="`${book.origin}-${book.bookUrl}`"
          class="result-item"
          @click="openBook(book)"
        >
          <div class="result-main">
            <p class="result-name" :title="book.name">{{ book.name }}</p>
            <p class="result-sub">
              <span class="result-author">{{ book.author || '佚名' }}</span>
              <span class="source-badge" :title="book.originName || book.origin">{{ book.originName || book.origin }}</span>
              <span v-if="book.latestChapterTitle" class="result-chapter" :title="book.latestChapterTitle">
                {{ book.latestChapterTitle }}
              </span>
            </p>
            <p v-if="book.intro" class="result-intro">{{ book.intro }}</p>
          </div>
          <svg class="result-arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M9 6l6 6-6 6" />
          </svg>
        </li>
      </ul>
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
.result-list {
  list-style: none;
  margin: 22px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}
.result-item {
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
  gap: 10px;
  margin: 5px 0 0;
  min-width: 0;
}
.result-author {
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
  flex-shrink: 0;
}
/* 来源徽标：细字描边 */
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
