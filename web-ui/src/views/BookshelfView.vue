<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import LogoMark from '@/components/LogoMark.vue'
import { getBookshelf } from '@/api/bookshelf'
import { useUserStore } from '@/stores/user'
import type { Book } from '@/types'

const router = useRouter()
const store = useUserStore()

const books = ref<Book[]>([])
const loading = ref(true)
const refreshing = ref(false)
const keyword = ref('')
const failedCovers = ref<Set<string>>(new Set())

/** 封面占位 = 莫兰迪低饱和纯色块（按书名 hash 取色） */
const MORANDI = [
  '#9aa8a0', // 鼠尾草绿
  '#a5a0b0', // 雾紫灰
  '#b0a59a', // 暖沙
  '#9aa8b5', // 雾蓝灰
  '#b0a0a5', // 灰玫瑰
  '#a3a79a', // 橄榄灰
  '#a89fb0', // 藕荷灰
  '#b5a89c', // 陶土灰
]

function hashName(name: string): number {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return h
}

function coverColor(name: string): string {
  return MORANDI[hashName(name) % MORANDI.length]
}

function coverSrc(book: Book): string | null {
  return book.customCoverUrl || book.coverUrl || null
}

function hasCover(book: Book): boolean {
  const src = coverSrc(book)
  return !!src && !failedCovers.value.has(book.bookUrl)
}

function onCoverError(book: Book) {
  failedCovers.value.add(book.bookUrl)
}

function coverInitial(name: string): string {
  const ch = name.trim().charAt(0)
  return ch ? ch.toUpperCase() : '书'
}

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  if (!kw) return books.value
  return books.value.filter(
    (b) => b.name.toLowerCase().includes(kw) || b.author.toLowerCase().includes(kw),
  )
})

async function load(silent = false) {
  if (!silent) loading.value = true
  else refreshing.value = true
  try {
    const res = await getBookshelf()
    books.value = res.data ?? []
  } catch {
    // 错误提示已由拦截器统一处理
  } finally {
    loading.value = false
    refreshing.value = false
  }
}

function logout() {
  store.clear()
  void router.replace('/login')
}

function openBook(book: Book) {
  void router.push(`/book/${encodeURIComponent(book.bookUrl)}`)
}

onMounted(() => load())
</script>

<template>
  <div class="bookshelf-page">
    <!-- 极简导航：字标 + 搜索 + 用户 -->
    <header class="topbar">
      <div class="brand">
        <LogoMark class="brand-logo" />
        <span class="brand-name">夜读<span class="brand-dot">.</span></span>
      </div>

      <div class="search-box">
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
          v-model="keyword"
          class="search-input"
          type="text"
          placeholder="搜索书名 / 作者"
          spellcheck="false"
        />
        <button
          v-if="keyword"
          class="search-clear"
          type="button"
          title="清空"
          @click="keyword = ''"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="user-area">
        <span class="user-chip">{{ store.username || '未登录' }}</span>
        <button class="logout-btn" type="button" @click="logout">退出</button>
      </div>
    </header>

    <main class="content">
      <!-- 标题区 -->
      <div class="section-head">
        <h1 class="section-title">我的书架</h1>
        <span class="count">{{ books.length }} 本</span>
        <button
          class="refresh-btn"
          type="button"
          title="刷新书架"
          :class="{ spinning: refreshing }"
          @click="load(true)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12a9 9 0 1 1-2.64-6.36" />
            <path d="M21 3v6h-6" />
          </svg>
        </button>
      </div>

      <!-- 加载骨架（浅灰静置块） -->
      <div v-if="loading" class="book-grid" aria-label="加载中">
        <div v-for="i in 12" :key="i" class="skeleton-card">
          <div class="skeleton-cover"></div>
          <div class="skeleton-line"></div>
          <div class="skeleton-line short"></div>
        </div>
      </div>

      <!-- 空状态 -->
      <div v-else-if="filtered.length === 0" class="empty-state">
        <p class="empty-text">
          {{ keyword ? '没有找到匹配的书籍' : '书架空空如也，去搜索添加第一本书吧' }}
        </p>
      </div>

      <!-- 书封网格（大间距） -->
      <div v-else class="book-grid">
        <div v-for="book in filtered" :key="book.bookUrl" class="book-card" @click="openBook(book)">
          <div class="cover-wrap">
            <img
              v-if="hasCover(book)"
              v-lazy="coverSrc(book) as string"
              class="cover-img"
              :alt="book.name"
              loading="lazy"
              @error="onCoverError(book)"
            />
            <div v-else class="cover-ph" :style="{ background: coverColor(book.name) }">
              <span class="cover-ph-char">{{ coverInitial(book.name) }}</span>
            </div>
          </div>
          <div class="book-meta">
            <p class="book-name" :title="book.name">{{ book.name }}</p>
            <p class="book-author">{{ book.author || '佚名' }}</p>
            <p v-if="book.latestChapterTitle" class="book-chapter" :title="book.latestChapterTitle">
              {{ book.latestChapterTitle }}
            </p>
          </div>
        </div>
      </div>
    </main>
  </div>
</template>

<style scoped>
.bookshelf-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  animation: fade-in 0.2s ease both;
}

/* ================= 顶部导航 ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 24px;
  padding: 14px 32px;
  background: rgba(250, 250, 250, 0.86);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}

.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.brand-logo {
  width: 26px;
  height: 26px;
}
.brand-name {
  font-size: 17px;
  font-weight: 300;
  letter-spacing: 3px;
  color: var(--text-1);
}
.brand-dot {
  color: var(--accent);
  font-weight: 400;
}

/* 搜索框（细边框圆角 8px） */
.search-box {
  position: relative;
  flex: 1;
  max-width: 320px;
  margin: 0 auto;
}
.search-icon {
  position: absolute;
  left: 12px;
  top: 50%;
  transform: translateY(-50%);
  width: 15px;
  height: 15px;
  color: var(--text-3);
  pointer-events: none;
  transition: color 0.2s ease;
}
.search-box:focus-within .search-icon {
  color: var(--accent);
}
.search-input {
  width: 100%;
  height: 38px;
  padding: 0 34px 0 36px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-1);
  font-family: inherit;
  font-size: 13.5px;
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
}
.search-clear {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 50%;
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition: color 0.2s ease;
}
.search-clear:hover {
  color: var(--text-1);
}
.search-clear svg {
  width: 11px;
  height: 11px;
}

/* 用户区 */
.user-area {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-shrink: 0;
}
.user-chip {
  font-size: 13px;
  font-weight: 400;
  color: var(--text-2);
}
.logout-btn {
  padding: 6px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.logout-btn:hover {
  color: var(--text-1);
  border-color: var(--border-strong);
}

/* ================= 内容区 ================= */
.content {
  width: min(1200px, 100%);
  margin: 0 auto;
  padding: 48px 32px 72px;
}

.section-head {
  display: flex;
  align-items: baseline;
  gap: 14px;
  margin-bottom: 36px;
}
.section-title {
  margin: 0;
  font-size: 22px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-1);
}
.count {
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
}
.refresh-btn {
  margin-left: auto;
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.refresh-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.refresh-btn.spinning svg {
  animation: spin 0.8s linear infinite;
}
.refresh-btn svg {
  width: 14px;
  height: 14px;
}

/* ================= 书封网格（大间距 28-32px） ================= */
.book-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 32px 28px;
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
  opacity: 0;
  transition: opacity 0.3s ease;
}
.cover-img.is-loaded {
  opacity: 1;
}

/* 莫兰迪纯色占位 + 细体首字 */
.cover-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}
.cover-ph-char {
  font-size: 44px;
  font-weight: 300;
  color: rgba(255, 255, 255, 0.94);
  letter-spacing: 2px;
}

/* 书籍信息 */
.book-meta {
  padding: 12px 2px 0;
}
.book-name {
  margin: 0;
  font-size: 14px;
  font-weight: 500;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-author {
  margin: 4px 0 0;
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-chapter {
  margin: 6px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ================= 骨架屏（浅灰静置） ================= */
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

/* ================= 空状态 ================= */
.empty-state {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 96px 0;
}
.empty-text {
  margin: 0;
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}

/* 旋转（刷新按钮 / 登录 spinner 共用） */
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    flex-wrap: wrap;
    gap: 12px;
    padding: 12px 16px;
  }
  .search-box {
    order: 3;
    max-width: none;
    flex-basis: 100%;
    margin: 0;
  }
  .content {
    padding: 32px 16px 56px;
  }
  .book-grid {
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 24px 16px;
  }
  .section-head {
    margin-bottom: 28px;
  }
}
</style>
