<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getBookshelf } from '@/api/bookshelf'
import { getBookInfo } from '@/api/books'
import type { Book, BookInfo } from '@/types'

const route = useRoute()
const router = useRouter()

/** /book/:url —— vue-router 已自动解码 */
const bookUrl = computed(() => String(route.params.url ?? ''))

const shelfBook = ref<Book | null>(null)
const info = ref<BookInfo | null>(null)
const loading = ref(true)
const loadFailed = ref(false)
const coverFailed = ref(false)

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

async function load() {
  loading.value = true
  loadFailed.value = false
  try {
    // 详情接口需要 bookSource=book.origin，先从书架定位本书
    const res = await getBookshelf()
    const found = (res.data ?? []).find((b) => b.bookUrl === bookUrl.value) ?? null
    shelfBook.value = found
    if (found?.origin) {
      try {
        const infoRes = await getBookInfo(bookUrl.value, found.origin)
        if (infoRes.isSuccess) info.value = infoRes.data
      } catch {
        // 实时详情失败：用书架数据兜底展示
      }
    }
    if (!found) loadFailed.value = true
  } catch {
    loadFailed.value = true
  } finally {
    loading.value = false
  }
}

function startReading() {
  if (!shelfBook.value) return
  void router.push(`/reader/${encodeURIComponent(shelfBook.value.bookUrl)}`)
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

      <!-- 错误态：不在书架 / 书架拉取失败 -->
      <div v-else-if="loadFailed" class="empty-state">
        <p class="empty-text">未找到这本书（可能不在书架中）</p>
        <button class="ghost-btn" type="button" @click="router.push('/')">返回书架</button>
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
            <button class="read-btn" type="button" @click="startReading">开始阅读</button>
          </div>
        </div>
      </div>
    </main>
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
