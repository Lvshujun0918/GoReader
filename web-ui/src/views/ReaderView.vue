<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getBookshelf } from '@/api/bookshelf'
import { getBookInfo, getBookToc, getBookContent } from '@/api/books'
import type { Book, BookChapter } from '@/types'

const route = useRoute()
const router = useRouter()

/** /reader/:bookUrl —— vue-router 已自动解码 encodeURIComponent 参数 */
const bookUrl = computed(() => String(route.params.bookUrl ?? ''))

const MIN_FONT = 14
const MAX_FONT = 22
const FONT_KEY = 'reader_font_size'

/** 阅读进度：章节索引 + 滚动位置（localStorage: reader-progress-{bookUrl}） */
interface ReaderProgress {
  chapterIndex: number
  scrollY: number
  updatedAt: number
}

const shelfBook = ref<Book | null>(null)
const bookName = ref('')
const chapters = ref<BookChapter[]>([])
const chapterIndex = ref(0)
const content = ref('')
const loading = ref(true)
const loadError = ref(false)
const notFound = ref(false)
const drawerOpen = ref(false)

const fontSize = ref<number>(18)
{
  // 字号记忆（localStorage）
  const raw = Number(localStorage.getItem(FONT_KEY))
  if (raw >= MIN_FONT && raw <= MAX_FONT) fontSize.value = raw
}
watch(fontSize, (v) => {
  try {
    localStorage.setItem(FONT_KEY, String(v))
  } catch {
    /* ignore */
  }
})

/** 有效章节（跳过卷标题分隔行） */
const realChapters = computed(() => chapters.value.filter((c) => !c.isVolume))
const currentChapter = computed(() => chapters.value[chapterIndex.value] ?? null)
const flatIndex = computed(() =>
  currentChapter.value
    ? realChapters.value.findIndex((c) => c.url === currentChapter.value?.url)
    : -1,
)
const hasPrev = computed(() => flatIndex.value > 0)
const hasNext = computed(() => flatIndex.value >= 0 && flatIndex.value < realChapters.value.length - 1)

const paragraphs = computed(() =>
  content.value
    .split(/\n+/)
    .map((s) => s.trim())
    .filter(Boolean),
)

/** 首次进入需要恢复的滚动位置（正文渲染完成后应用一次） */
let restoreScrollY: number | null = null
let saveTimer: number | undefined

/* ---------------- 进度存取 ---------------- */

function progressKey(): string {
  return `reader-progress-${bookUrl.value}`
}

function saveProgress() {
  if (!currentChapter.value) return
  try {
    localStorage.setItem(
      progressKey(),
      JSON.stringify({
        chapterIndex: chapterIndex.value,
        scrollY: window.scrollY,
        updatedAt: Date.now(),
      } satisfies ReaderProgress),
    )
  } catch {
    /* ignore */
  }
}

function restoreProgress(): ReaderProgress | null {
  try {
    const raw = localStorage.getItem(progressKey())
    if (!raw) return null
    const p = JSON.parse(raw) as ReaderProgress
    if (typeof p.chapterIndex === 'number' && typeof p.scrollY === 'number') return p
  } catch {
    /* ignore */
  }
  return null
}

/* ---------------- 正文加载 ---------------- */

async function loadContent(chapterUrl: string) {
  if (!shelfBook.value?.origin) return
  loading.value = true
  loadError.value = false
  content.value = ''
  try {
    const res = await getBookContent(chapterUrl, shelfBook.value.origin)
    content.value = res.data?.content ?? ''
  } catch {
    loadError.value = true
    return
  } finally {
    loading.value = false
  }
  // 等正文真正渲染（loading 置 false 后）再滚动，避免被加载态高度钳制
  await nextTick()
  window.scrollTo(0, restoreScrollY ?? 0)
  restoreScrollY = null
}

function goToChapter(idx: number) {
  const ch = chapters.value[idx]
  if (!ch || ch.isVolume) return
  drawerOpen.value = false
  if (idx === chapterIndex.value) return
  saveProgress()
  chapterIndex.value = idx
  void loadContent(ch.url)
}

function prevChapter() {
  const fi = flatIndex.value
  if (fi <= 0) return
  goToChapter(chapters.value.indexOf(realChapters.value[fi - 1]))
}

function nextChapter() {
  const fi = flatIndex.value
  if (fi < 0 || fi >= realChapters.value.length - 1) return
  goToChapter(chapters.value.indexOf(realChapters.value[fi + 1]))
}

function retry() {
  if (chapters.value.length === 0) void init()
  else if (currentChapter.value) void loadContent(currentChapter.value.url)
}

/* ---------------- 初始化 ---------------- */

async function init() {
  loading.value = true
  loadError.value = false
  notFound.value = false
  try {
    // 正文/目录接口需要 bookSource=book.origin，先从书架定位本书
    const shelfRes = await getBookshelf()
    const found = (shelfRes.data ?? []).find((b) => b.bookUrl === bookUrl.value)
    if (!found?.origin || !found.tocUrl) {
      notFound.value = true
      return
    }
    shelfBook.value = found
    bookName.value = found.name

    // 目录 + 详情并行拉取
    const [tocRes, infoRes] = await Promise.allSettled([
      getBookToc(found.tocUrl, found.origin),
      getBookInfo(found.bookUrl, found.origin),
    ])
    if (tocRes.status === 'fulfilled' && tocRes.value.isSuccess) {
      chapters.value = tocRes.value.data ?? []
    } else {
      loadError.value = true
      return
    }
    if (infoRes.status === 'fulfilled' && infoRes.value.isSuccess && infoRes.value.data) {
      bookName.value = infoRes.value.data.name || bookName.value
    }

    // 起始章节：恢复进度优先，否则第一章
    const saved = restoreProgress()
    let startIndex = realChapters.value.length ? chapters.value.indexOf(realChapters.value[0]) : 0
    if (
      saved &&
      saved.chapterIndex >= 0 &&
      saved.chapterIndex < chapters.value.length &&
      !chapters.value[saved.chapterIndex].isVolume
    ) {
      startIndex = saved.chapterIndex
      restoreScrollY = saved.scrollY
    }
    chapterIndex.value = startIndex
    const start = chapters.value[startIndex]
    if (start) await loadContent(start.url)
  } catch {
    loadError.value = true
  } finally {
    loading.value = false
  }
}

function goBack() {
  if (window.history.length > 1) router.back()
  else void router.replace(`/book/${encodeURIComponent(bookUrl.value)}`)
}

/* ---------------- 生命周期 ---------------- */

function onScroll() {
  window.clearTimeout(saveTimer)
  saveTimer = window.setTimeout(saveProgress, 300)
}

onMounted(() => {
  window.addEventListener('scroll', onScroll, { passive: true })
  void init()
})

onBeforeUnmount(() => {
  window.removeEventListener('scroll', onScroll)
  window.clearTimeout(saveTimer)
  saveProgress()
})
</script>

<template>
  <div class="reader-page">
    <!-- 顶部极简栏 -->
    <header class="topbar">
      <button class="icon-btn" type="button" title="返回" @click="goBack">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5" />
          <path d="M11 18l-6-6 6-6" />
        </svg>
      </button>

      <span class="book-name" :title="bookName">{{ bookName || '阅读' }}</span>

      <div class="top-actions">
        <button
          class="font-btn"
          type="button"
          :disabled="fontSize <= MIN_FONT"
          title="减小字号"
          @click="fontSize = Math.max(MIN_FONT, fontSize - 1)"
        >
          A-
        </button>
        <button
          class="font-btn"
          type="button"
          :disabled="fontSize >= MAX_FONT"
          title="增大字号"
          @click="fontSize = Math.min(MAX_FONT, fontSize + 1)"
        >
          A+
        </button>
        <button class="toc-btn" type="button" title="目录" @click="drawerOpen = true">
          目录
        </button>
      </div>
    </header>

    <!-- 正文 -->
    <main class="reader-main">
      <!-- 不在书架 -->
      <div v-if="notFound" class="state">
        <p class="state-text">未找到这本书（可能不在书架中）</p>
        <button class="retry-btn" type="button" @click="router.replace('/')">返回书架</button>
      </div>

      <!-- 目录为空 -->
      <div v-else-if="!loading && !loadError && chapters.length === 0" class="state">
        <p class="state-text">未获取到章节目录</p>
        <button class="retry-btn" type="button" @click="retry">重试</button>
      </div>

      <template v-else>
        <h1 v-if="currentChapter" class="chapter-title">{{ currentChapter.title }}</h1>

        <!-- 加载态：细字 -->
        <div v-if="loading" class="state">
          <p class="state-text loading-text">加载中…</p>
        </div>

        <!-- 错误态 -->
        <div v-else-if="loadError" class="state">
          <p class="state-text">正文获取失败，请稍后重试</p>
          <button class="retry-btn" type="button" @click="retry">重试</button>
        </div>

        <!-- 空内容 -->
        <div v-else-if="paragraphs.length === 0" class="state">
          <p class="state-text">本章暂无内容</p>
        </div>

        <!-- 正文 -->
        <article v-else class="reader-content" :style="{ fontSize: `${fontSize}px` }">
          <p v-for="(para, i) in paragraphs" :key="i" class="reader-para">{{ para }}</p>
        </article>

        <!-- 底部极简导航 -->
        <nav class="chapter-nav">
          <button class="nav-btn" type="button" :disabled="!hasPrev" @click="prevChapter">
            上一章
          </button>
          <button class="nav-btn" type="button" :disabled="!hasNext" @click="nextChapter">
            下一章
          </button>
        </nav>
      </template>
    </main>

    <!-- 章节侧栏 -->
    <transition name="drawer">
      <div v-if="drawerOpen" class="drawer-mask" @click="drawerOpen = false">
        <aside class="chapter-drawer" @click.stop>
          <header class="drawer-head">
            <span class="drawer-title">目录</span>
            <button class="drawer-close" type="button" title="关闭" @click="drawerOpen = false">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                <path d="M6 6l12 12M18 6L6 18" />
              </svg>
            </button>
          </header>
          <div class="drawer-list">
            <template v-for="(ch, i) in chapters" :key="`${ch.url}-${i}`">
              <div v-if="ch.isVolume" class="chapter-volume">{{ ch.title }}</div>
              <button
                v-else
                type="button"
                class="chapter-item"
                :class="{ active: i === chapterIndex }"
                @click="goToChapter(i)"
              >
                {{ ch.title }}
              </button>
            </template>
          </div>
        </aside>
      </div>
    </transition>
  </div>
</template>

<style scoped>
.reader-page {
  min-height: 100vh;
  animation: fade-in 0.2s ease both;
}

/* ================= 顶部极简栏 ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 12px 24px;
  background: rgba(250, 250, 250, 0.86);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}
.icon-btn {
  flex-shrink: 0;
  width: 34px;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid transparent;
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.icon-btn:hover {
  color: var(--text-1);
  border-color: var(--border);
}
.icon-btn svg {
  width: 16px;
  height: 16px;
}
.book-name {
  flex: 1;
  min-width: 0;
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  text-align: center;
}
.top-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}
.font-btn {
  min-width: 34px;
  height: 30px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.font-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.font-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.toc-btn {
  height: 30px;
  padding: 0 12px;
  margin-left: 4px;
  border: none;
  border-radius: var(--radius);
  background: var(--accent-soft);
  color: var(--accent);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition: background 0.2s ease;
}
.toc-btn:hover {
  background: rgba(79, 70, 229, 0.14);
}

/* ================= 正文 ================= */
.reader-main {
  width: min(680px, 100%);
  margin: 0 auto;
  padding: 48px 24px 96px;
}

.chapter-title {
  margin: 0 0 36px;
  font-size: 20px;
  font-weight: 300;
  letter-spacing: 2px;
  text-align: center;
  color: var(--text-1);
}

.reader-content {
  color: var(--text-1);
}
.reader-para {
  margin: 0 0 1em;
  line-height: 1.9;
  font-weight: 400;
  text-indent: 2em;
  word-break: break-word;
}

/* ================= 加载 / 错误 / 空 ================= */
.state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 20px;
  padding: 80px 0;
}
.state-text {
  margin: 0;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-3);
}
.loading-text {
  animation: pulse 1.2s ease-in-out infinite;
}
@keyframes pulse {
  0%,
  100% {
    opacity: 0.45;
  }
  50% {
    opacity: 1;
  }
}
.retry-btn {
  padding: 8px 30px;
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
.retry-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
}

/* ================= 底部导航 ================= */
.chapter-nav {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 16px;
  margin-top: 64px;
  padding-top: 32px;
  border-top: 1px solid var(--border);
}
.nav-btn {
  height: 42px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 3px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.nav-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.nav-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

/* ================= 章节侧栏 ================= */
.drawer-mask {
  position: fixed;
  inset: 0;
  z-index: 50;
  background: rgba(24, 24, 27, 0.32);
}
.chapter-drawer {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(320px, 86vw);
  display: flex;
  flex-direction: column;
  background: var(--surface);
  border-left: 1px solid var(--border);
}
.drawer-enter-active,
.drawer-leave-active {
  transition: opacity 0.2s ease;
}
.drawer-enter-active .chapter-drawer,
.drawer-leave-active .chapter-drawer {
  transition: transform 0.2s ease;
}
.drawer-enter-from,
.drawer-leave-to {
  opacity: 0;
}
.drawer-enter-from .chapter-drawer,
.drawer-leave-to .chapter-drawer {
  transform: translateX(100%);
}

.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
}
.drawer-title {
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 3px;
  color: var(--text-1);
}
.drawer-close {
  width: 28px;
  height: 28px;
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
.drawer-close:hover {
  color: var(--text-1);
}
.drawer-close svg {
  width: 13px;
  height: 13px;
}

.drawer-list {
  flex: 1;
  overflow-y: auto;
  padding: 10px 0 24px;
}
.chapter-volume {
  padding: 20px 20px 8px;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-3);
}
.chapter-item {
  display: block;
  width: 100%;
  padding: 11px 20px;
  border: none;
  border-left: 2px solid transparent;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  text-align: left;
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition:
    color 0.2s ease,
    background 0.2s ease,
    border-color 0.2s ease;
}
.chapter-item:hover {
  color: var(--text-1);
  background: #f4f4f5;
}
.chapter-item.active {
  color: var(--accent);
  border-left-color: var(--accent);
  background: var(--accent-soft);
  font-weight: 400;
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    padding: 10px 14px;
  }
  .reader-main {
    padding: 36px 20px 72px;
  }
  .chapter-nav {
    gap: 12px;
  }
}
</style>
