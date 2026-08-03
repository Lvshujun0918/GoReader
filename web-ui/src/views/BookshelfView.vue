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

/** 缺失封面时的渐变占位（按书名 hash 从 6 组配色中选取） */
const COVER_GRADIENTS = [
  'linear-gradient(150deg, #6366f1 0%, #a855f7 100%)',
  'linear-gradient(150deg, #0ea5e9 0%, #22d3ee 100%)',
  'linear-gradient(150deg, #f43f5e 0%, #fb923c 100%)',
  'linear-gradient(150deg, #10b981 0%, #34d399 100%)',
  'linear-gradient(150deg, #8b5cf6 0%, #ec4899 100%)',
  'linear-gradient(150deg, #f59e0b 0%, #ef4444 100%)',
]

function hashName(name: string): number {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return h
}

function coverGradient(name: string): string {
  return COVER_GRADIENTS[hashName(name) % COVER_GRADIENTS.length]
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

onMounted(() => load())
</script>

<template>
  <div class="bookshelf-page">
    <!-- 顶部导航栏 -->
    <header class="topbar glass">
      <div class="brand">
        <LogoMark class="brand-logo" />
        <span class="brand-name">夜读</span>
        <span class="brand-sub">Reader</span>
      </div>

      <div class="search-box">
        <svg class="search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="11" cy="11" r="6.5" />
          <path d="M20 20l-3.8-3.8" />
        </svg>
        <input
          v-model="keyword"
          class="search-input"
          type="text"
          placeholder="搜索书名 / 作者…"
          spellcheck="false"
        />
        <button v-if="keyword" class="search-clear" type="button" title="清空" @click="keyword = ''">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="user-area">
        <span class="user-chip">
          <svg class="user-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round">
            <circle cx="12" cy="8" r="3.6" />
            <path d="M4.5 20c1.4-3.4 4.3-5 7.5-5s6.1 1.6 7.5 5" />
          </svg>
          {{ store.username || '未登录' }}
        </span>
        <button class="logout-btn" type="button" @click="logout">退出</button>
      </div>
    </header>

    <main class="content">
      <!-- 标题区 -->
      <div class="section-head fade-up">
        <h1 class="section-title">我的书架</h1>
        <span class="count-badge">{{ books.length }} 本</span>
        <button
          class="refresh-btn"
          type="button"
          title="刷新书架"
          :class="{ spinning: refreshing }"
          @click="load(true)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12a9 9 0 1 1-2.64-6.36" />
            <path d="M21 3v6h-6" />
          </svg>
        </button>
      </div>

      <!-- 加载骨架 -->
      <div v-if="loading" class="book-grid" aria-label="加载中">
        <div v-for="i in 12" :key="i" class="skeleton-card">
          <div class="skeleton-cover"></div>
          <div class="skeleton-line"></div>
          <div class="skeleton-line short"></div>
        </div>
      </div>

      <!-- 空状态 -->
      <div v-else-if="filtered.length === 0" class="empty-state fade-up">
        <div class="empty-icon">📚</div>
        <p class="empty-text">{{ keyword ? '没有找到匹配的书籍' : '书架空空如也，去搜索添加第一本书吧' }}</p>
      </div>

      <!-- 书封网格 -->
      <div v-else class="book-grid">
        <div
          v-for="(book, i) in filtered"
          :key="book.bookUrl"
          class="book-card fade-up"
          :style="{ animationDelay: `${Math.min(i, 24) * 45}ms` }"
        >
          <div class="cover-wrap">
            <img
              v-if="hasCover(book)"
              v-lazy="coverSrc(book) as string"
              class="cover-img"
              :alt="book.name"
              loading="lazy"
              @error="onCoverError(book)"
            />
            <div v-else class="cover-ph" :style="{ background: coverGradient(book.name) }">
              <span class="cover-ph-char">{{ coverInitial(book.name) }}</span>
              <span class="cover-ph-shine"></span>
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
}

/* ================= 顶部导航栏 ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 20px;
  padding: 12px 28px;
  border-left: none;
  border-right: none;
  border-top: none;
  border-radius: 0;
  background: rgba(13, 16, 24, 0.72);
  animation: fade-in 0.5s ease both;
}

.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.brand-logo {
  width: 34px;
  height: 34px;
}
.brand-name {
  font-size: 19px;
  font-weight: 700;
  letter-spacing: 3px;
  background: linear-gradient(120deg, #e0e7ff, #a5b4fc 55%, #67e8f9);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}
.brand-sub {
  font-size: 12px;
  letter-spacing: 1.5px;
  color: var(--text-3);
  align-self: flex-end;
  margin-bottom: 3px;
}

/* 搜索框 */
.search-box {
  position: relative;
  flex: 1;
  max-width: 380px;
  margin: 0 auto;
}
.search-icon {
  position: absolute;
  left: 13px;
  top: 50%;
  transform: translateY(-50%);
  width: 16px;
  height: 16px;
  color: var(--text-3);
  pointer-events: none;
  transition: color 0.3s;
}
.search-box:focus-within .search-icon {
  color: #a5b4fc;
}
.search-input {
  width: 100%;
  height: 40px;
  padding: 0 38px 0 38px;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-1);
  font-size: 13.5px;
  outline: none;
  transition:
    border-color 0.3s,
    box-shadow 0.3s,
    background 0.3s;
}
.search-input::placeholder {
  color: var(--text-3);
}
.search-input:focus {
  border-color: var(--brand-1);
  background: rgba(255, 255, 255, 0.07);
  box-shadow: 0 0 0 3.5px rgba(99, 102, 241, 0.16);
}
.search-clear {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.1);
  color: var(--text-2);
  cursor: pointer;
  transition: background 0.25s, color 0.25s;
}
.search-clear:hover {
  background: rgba(255, 255, 255, 0.18);
  color: var(--text-1);
}
.search-clear svg {
  width: 12px;
  height: 12px;
}

/* 用户区 */
.user-area {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.user-chip {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 7px 14px;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-2);
  font-size: 13px;
}
.user-icon {
  width: 14px;
  height: 14px;
  color: #a5b4fc;
}
.logout-btn {
  padding: 7px 14px;
  border-radius: 999px;
  border: 1px solid rgba(244, 63, 94, 0.35);
  background: rgba(244, 63, 94, 0.08);
  color: #fda4af;
  font-size: 13px;
  cursor: pointer;
  transition: all 0.25s;
}
.logout-btn:hover {
  background: rgba(244, 63, 94, 0.18);
  border-color: rgba(244, 63, 94, 0.6);
}

/* ================= 内容区 ================= */
.content {
  width: min(1240px, 100%);
  margin: 0 auto;
  padding: 34px 28px 60px;
}

.section-head {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 26px;
}
.section-title {
  margin: 0;
  font-size: 24px;
  font-weight: 700;
  letter-spacing: 2px;
}
.count-badge {
  padding: 3px 12px;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: var(--glass);
  color: var(--text-2);
  font-size: 12.5px;
}
.refresh-btn {
  margin-left: auto;
  width: 34px;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  border: 1px solid var(--glass-border);
  background: var(--glass);
  color: var(--text-2);
  cursor: pointer;
  transition: all 0.25s;
}
.refresh-btn:hover {
  color: var(--text-1);
  border-color: rgba(255, 255, 255, 0.22);
  transform: rotate(90deg);
}
.refresh-btn.spinning svg {
  animation: spin 0.8s linear infinite;
}
.refresh-btn svg {
  width: 15px;
  height: 15px;
}

/* ================= 书封网格 ================= */
.book-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(152px, 1fr));
  gap: 26px 20px;
}

.book-card {
  cursor: pointer;
  transition: transform 0.35s var(--ease-out);
}
.book-card:hover {
  transform: translateY(-6px);
}

.cover-wrap {
  position: relative;
  aspect-ratio: 3 / 4;
  border-radius: 14px;
  overflow: hidden;
  background: rgba(255, 255, 255, 0.04);
  box-shadow: var(--shadow-card);
  transition: box-shadow 0.35s;
}
.book-card:hover .cover-wrap {
  box-shadow:
    0 22px 44px -16px rgba(0, 0, 0, 0.75),
    0 0 0 1px rgba(255, 255, 255, 0.14),
    0 0 34px -8px rgba(99, 102, 241, 0.5);
}

.cover-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
  opacity: 0;
  transform: scale(1.02);
  transition: opacity 0.55s ease, transform 0.55s var(--ease-out);
}
.cover-img.is-loaded {
  opacity: 1;
  transform: scale(1);
}
.book-card:hover .cover-img {
  transform: scale(1.05);
}

/* 渐变占位封面 */
.cover-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}
.cover-ph-char {
  font-size: 46px;
  font-weight: 700;
  color: rgba(255, 255, 255, 0.92);
  text-shadow: 0 4px 18px rgba(0, 0, 0, 0.35);
  letter-spacing: 2px;
}
.cover-ph-shine {
  position: absolute;
  inset: 0;
  background: linear-gradient(160deg, rgba(255, 255, 255, 0.22) 0%, transparent 38%);
}

/* 书籍信息 */
.book-meta {
  padding: 10px 4px 0;
}
.book-name {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-author {
  margin: 3px 0 0;
  font-size: 12px;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.book-chapter {
  margin: 5px 0 0;
  font-size: 11.5px;
  color: rgba(165, 180, 252, 0.75);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ================= 骨架屏 ================= */
.skeleton-card {
  animation: fade-in 0.4s ease both;
}
.skeleton-cover {
  aspect-ratio: 3 / 4;
  border-radius: 14px;
  background: linear-gradient(90deg, rgba(255, 255, 255, 0.045) 25%, rgba(255, 255, 255, 0.1) 50%, rgba(255, 255, 255, 0.045) 75%);
  background-size: 840px 100%;
  animation: shimmer 1.6s infinite linear;
}
.skeleton-line {
  height: 12px;
  margin-top: 12px;
  border-radius: 6px;
  background: linear-gradient(90deg, rgba(255, 255, 255, 0.05) 25%, rgba(255, 255, 255, 0.11) 50%, rgba(255, 255, 255, 0.05) 75%);
  background-size: 840px 100%;
  animation: shimmer 1.6s infinite linear;
}
.skeleton-line.short {
  width: 55%;
  margin-top: 8px;
}

/* ================= 空状态 ================= */
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 90px 0;
}
.empty-icon {
  font-size: 52px;
  opacity: 0.85;
  filter: drop-shadow(0 10px 24px rgba(99, 102, 241, 0.35));
  animation: float-soft 6s ease-in-out infinite alternate;
}
.empty-text {
  margin: 18px 0 0;
  font-size: 14px;
  color: var(--text-3);
  letter-spacing: 1px;
}

/* 空状态图标浮动 */
@keyframes float-soft {
  from {
    transform: translateY(0);
  }
  to {
    transform: translateY(-12px);
  }
}

/* 响应式 */
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
  }
  .content {
    padding: 24px 16px 48px;
  }
  .book-grid {
    grid-template-columns: repeat(auto-fill, minmax(128px, 1fr));
    gap: 20px 14px;
  }
}
</style>
