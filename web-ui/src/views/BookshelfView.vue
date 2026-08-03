<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import {
  deleteBook,
  getBookGroups,
  getBookshelf,
  saveBookGroup,
  updateBookGroupId,
} from '@/api/bookshelf'
import { uploadLocalBook } from '@/api/upload'
import { useUserStore } from '@/stores/user'
import type { Book, BookGroup } from '@/types'

const router = useRouter()
const store = useUserStore()

const books = ref<Book[]>([])
const loading = ref(true)
const refreshing = ref(false)
const keyword = ref('')
const failedCovers = ref<Set<string>>(new Set())

/* ================= 书架分组 ================= */
const groups = ref<BookGroup[]>([])
const activeGroup = ref<number | null>(null) // null=全部
const groupOpen = ref(false)
const groupDialogRef = ref<HTMLElement | null>(null)
const newGroupName = ref('')
const groupSaving = ref(false)

/* ================= 书卡菜单（右键 / 长按 / hover ⋯） ================= */
const menuBook = ref<Book | null>(null)
const menuPos = ref({ x: 0, y: 0 })
const menuOpen = ref(false)
const movePanel = ref(false)
const menuBusy = ref(false)
let longPressTimer: number | undefined
let longPressFired = false
let suppressClick = false

/* ================= 导入本地书 ================= */
interface ImportItem {
  file: File
  status: 'pending' | 'uploading' | 'done' | 'error'
  progress: number
  error?: string
}

const importOpen = ref(false)
const dialogRef = ref<HTMLElement | null>(null)
const fileInput = ref<HTMLInputElement | null>(null)
const isDragOver = ref(false)
const uploadBusy = ref(false)
const uploadIndex = ref(0)
const importDone = ref(false)
const importSummary = ref('')
const acceptTip = ref('')
const importItems = ref<ImportItem[]>([])

/** 整体进度：按文件大小加权 */
const totalProgress = computed(() => {
  const items = importItems.value
  if (!items.length) return 0
  const totalSize = items.reduce((s, it) => s + it.file.size, 0) || 1
  const loaded = items.reduce((s, it) => s + (it.file.size * it.progress) / 100, 0)
  return Math.min(99, Math.round((loaded / totalSize) * 100))
})
const hasPending = computed(() => importItems.value.some((it) => it.status === 'pending'))
const hasPendingCount = computed(() => importItems.value.filter((it) => it.status === 'pending').length)
const failedCount = computed(() => importItems.value.filter((it) => it.status === 'error').length)

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function isSupported(file: File): boolean {
  const name = file.name.toLowerCase()
  return (
    name.endsWith('.epub') ||
    name.endsWith('.txt') ||
    file.type === 'application/epub+zip' ||
    file.type === 'text/plain' ||
    file.type.startsWith('text/')
  )
}

function openImport() {
  importOpen.value = true
  uploadBusy.value = false
  importDone.value = false
  importSummary.value = ''
  acceptTip.value = ''
  importItems.value = []
  document.body.style.overflow = 'hidden'
  void nextTick(() => dialogRef.value?.focus())
}

function closeImport() {
  if (uploadBusy.value) return
  importOpen.value = false
  document.body.style.overflow = ''
}

function addFiles(files: File[]) {
  if (uploadBusy.value) return
  const valid = files.filter(isSupported)
  const ignored = files.length - valid.length
  for (const f of valid) importItems.value.push({ file: f, status: 'pending', progress: 0 })
  acceptTip.value = ignored > 0 ? `已忽略 ${ignored} 个不支持的文件（仅支持 .epub / .txt）` : ''
  if (valid.length > 0) {
    importDone.value = false
    importSummary.value = ''
  }
}

function onPick(e: Event) {
  const input = e.target as HTMLInputElement
  addFiles(Array.from(input.files ?? []))
  input.value = '' // 清空以便重复选择同一文件
}

function onDragOver(e: DragEvent) {
  e.preventDefault()
  isDragOver.value = true
}

function onDragLeave(e: DragEvent) {
  const cur = e.currentTarget as HTMLElement | null
  if (!cur || !cur.contains(e.relatedTarget as Node | null)) isDragOver.value = false
}

function onDrop(e: DragEvent) {
  e.preventDefault()
  isDragOver.value = false
  if (uploadBusy.value) return
  addFiles(Array.from(e.dataTransfer?.files ?? []))
}

function removeItem(i: number) {
  if (uploadBusy.value) return
  importItems.value.splice(i, 1)
}

/** 逐个上传（每个文件一次 multipart POST），完成后自动刷新书架 */
async function startUpload() {
  if (uploadBusy.value || importItems.value.length === 0) return
  uploadBusy.value = true
  importDone.value = false
  let ok = 0
  for (let i = 0; i < importItems.value.length; i++) {
    const item = importItems.value[i]
    uploadIndex.value = i
    item.status = 'uploading'
    item.progress = 0
    try {
      await uploadLocalBook(item.file, (p) => (item.progress = p))
      item.status = 'done'
      item.progress = 100
      ok++
    } catch (err) {
      item.status = 'error'
      item.error = err instanceof Error ? err.message : '导入失败'
    }
  }
  uploadBusy.value = false
  importDone.value = true
  const failed = importItems.value.length - ok
  importSummary.value =
    failed > 0 ? `导入完成：${ok} 本成功，${failed} 本失败` : `导入完成，共 ${ok} 本`
  await load() // 刷新书架（getBookshelf）
  if (failed === 0) window.setTimeout(() => closeImport(), 800)
}

onBeforeUnmount(() => {
  if (longPressTimer) clearTimeout(longPressTimer)
  document.body.style.overflow = ''
})

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
  const gid = activeGroup.value
  return books.value.filter((b) => {
    if (gid !== null && b.group !== gid) return false
    if (!kw) return true
    return b.name.toLowerCase().includes(kw) || b.author.toLowerCase().includes(kw)
  })
})

const emptyText = computed(() => {
  if (keyword.value) return '没有找到匹配的书籍'
  if (activeGroup.value !== null) return '该分组下暂无书籍'
  return '书架空空如也，去搜索添加第一本书吧'
})

async function load(silent = false) {
  if (!silent) loading.value = true
  else refreshing.value = true
  try {
    const [res, gRes] = await Promise.all([
      getBookshelf(),
      getBookGroups().catch(() => ({ isSuccess: false, errorMsg: '', data: [] as BookGroup[] })),
    ])
    books.value = res.data ?? []
    groups.value = gRes.data ?? []
    // 分组被删/失效时回退到「全部」
    if (activeGroup.value !== null && !groups.value.some((g) => g.id === activeGroup.value)) {
      activeGroup.value = null
    }
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

/* ================= 分组管理 ================= */
function groupName(id: number): string {
  return groups.value.find((g) => g.id === id)?.name ?? (id === 0 ? '未分组' : `分组 ${id}`)
}

function groupCount(id: number): number {
  return books.value.filter((b) => b.group === id).length
}

function openGroups() {
  groupOpen.value = true
  newGroupName.value = ''
  document.body.style.overflow = 'hidden'
  void nextTick(() => groupDialogRef.value?.focus())
}

function closeGroups() {
  if (groupSaving.value) return
  groupOpen.value = false
  document.body.style.overflow = ''
}

async function createGroup() {
  const name = newGroupName.value.trim()
  if (!name) return
  groupSaving.value = true
  try {
    const res = await saveBookGroup(name)
    groups.value.push(res.data)
    newGroupName.value = ''
    ElMessage.success('已新建分组')
  } catch {
    // 错误提示已由拦截器统一处理
  } finally {
    groupSaving.value = false
  }
}

/** 删除分组：后端暂无删除 API（TODO）。有书先提示移动，无书提示待实现 */
function deleteGroupHint(g: BookGroup) {
  const n = groupCount(g.id)
  if (n > 0) ElMessage.warning(`分组「${g.name}」内有 ${n} 本书，请先移动或移出后再删除`)
  else ElMessage.info('删除分组接口后端暂未提供（TODO）')
}

/* ================= 书卡菜单 ================= */
function openMenuAt(book: Book, x: number, y: number) {
  menuBook.value = book
  menuPos.value = {
    x: Math.min(Math.max(8, x), window.innerWidth - 190),
    y: Math.min(Math.max(8, y), window.innerHeight - 220),
  }
  movePanel.value = false
  menuOpen.value = true
}

function openCardMenu(book: Book, e: MouseEvent) {
  e.preventDefault()
  e.stopPropagation()
  openMenuAt(book, e.clientX, e.clientY)
}

function openMenuAtEl(book: Book, e: MouseEvent) {
  e.preventDefault()
  e.stopPropagation()
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
  openMenuAt(book, rect.left, rect.bottom + 6)
}

/** 触屏长按 500ms 唤出菜单（与点击进详情互斥） */
function onCardTouchStart(book: Book, e: TouchEvent) {
  longPressFired = false
  suppressClick = false
  const t = e.touches[0]
  longPressTimer = window.setTimeout(() => {
    longPressFired = true
    suppressClick = true
    openMenuAt(book, t.clientX, t.clientY)
  }, 500)
}

function onCardTouchEnd() {
  if (longPressTimer) {
    clearTimeout(longPressTimer)
    longPressTimer = undefined
  }
}

function onCardClick(book: Book) {
  if (longPressFired) {
    longPressFired = false
    return // 长按已触发菜单，忽略本次点击
  }
  openBook(book)
}

function closeMenu() {
  menuOpen.value = false
  menuBook.value = null
  movePanel.value = false
}

/** 长按后手指抬起产生的合成 click 会落在遮罩上，吞掉一次防止菜单秒关 */
function onOverlayClick() {
  if (suppressClick) {
    suppressClick = false
    return
  }
  closeMenu()
}

async function moveToGroup(groupId: number) {
  const book = menuBook.value
  if (!book || menuBusy.value) return
  menuBusy.value = true
  try {
    await updateBookGroupId(book.bookUrl, groupId)
    book.group = groupId
    ElMessage.success(groupId === 0 ? '已移出分组' : `已移动到「${groupName(groupId)}」`)
    closeMenu()
  } catch {
    // 错误提示已由拦截器统一处理
  } finally {
    menuBusy.value = false
  }
}

async function removeFromShelf() {
  const book = menuBook.value
  if (!book || menuBusy.value) return
  try {
    await ElMessageBox.confirm(`确定将《${book.name}》移出书架吗？`, '移出书架', {
      confirmButtonText: '移出',
      cancelButtonText: '取消',
      type: 'warning',
    })
  } catch {
    return // 用户取消
  }
  menuBusy.value = true
  try {
    await deleteBook(book.bookUrl)
    books.value = books.value.filter((b) => b.bookUrl !== book.bookUrl)
    ElMessage.success('已移出书架')
    closeMenu()
  } catch {
    // 错误提示已由拦截器统一处理
  } finally {
    menuBusy.value = false
  }
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
        <button class="nav-link" type="button" @click="router.push('/search')">搜索</button>
        <button class="nav-link" type="button" @click="router.push('/explore')">探索</button>
        <button class="nav-link" type="button" @click="router.push('/sources')">书源</button>
        <button class="nav-link" type="button" @click="router.push('/rules')">替换规则</button>
        <button class="nav-link" type="button" @click="router.push('/rss')">RSS</button>
        <button class="nav-link" type="button" @click="router.push('/files')">文件</button>
        <span class="user-chip">{{ store.username || '未登录' }}</span>
        <button class="logout-btn" type="button" @click="logout">退出</button>
      </div>
    </header>

    <main class="content">
      <!-- 标题区 -->
      <div class="section-head">
        <h1 class="section-title">我的书架</h1>
        <span class="count">{{ books.length }} 本</span>
        <button class="import-btn" type="button" title="导入本地书" @click="openImport">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 16V4" />
            <path d="M7 9l5-5 5 5" />
            <path d="M4 16v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3" />
          </svg>
          <span>导入本地书</span>
        </button>
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

      <!-- 分组栏：全部 / 分组名 胶囊筛选（细字，active 强调色下划线） -->
      <div class="group-bar">
        <div class="group-tabs" role="tablist" aria-label="书架分组筛选">
          <button
            type="button"
            class="group-tab"
            :class="{ active: activeGroup === null }"
            @click="activeGroup = null"
          >
            全部
          </button>
          <button
            v-for="g in groups"
            :key="g.id"
            type="button"
            class="group-tab"
            :class="{ active: activeGroup === g.id }"
            @click="activeGroup = g.id"
          >
            {{ g.name }}
          </button>
        </div>
        <button class="group-manage" type="button" @click="openGroups">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
            <path d="M4 7h10" />
            <path d="M18 7h2" />
            <circle cx="16" cy="7" r="2" />
            <path d="M4 17h2" />
            <path d="M10 17h10" />
            <circle cx="8" cy="17" r="2" />
          </svg>
          <span>管理</span>
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
        <p class="empty-text">{{ emptyText }}</p>
      </div>

      <!-- 书封网格（大间距） -->
      <div v-else class="book-grid">
        <div
          v-for="book in filtered"
          :key="book.bookUrl"
          class="book-card"
          @click="onCardClick(book)"
          @contextmenu="openCardMenu(book, $event)"
          @touchstart.passive="onCardTouchStart(book, $event)"
          @touchend="onCardTouchEnd"
          @touchcancel="onCardTouchEnd"
        >
          <button
            class="card-menu-btn"
            type="button"
            title="更多操作"
            @click="openMenuAtEl(book, $event)"
          >
            <svg viewBox="0 0 24 24" fill="currentColor">
              <circle cx="5" cy="12" r="1.6" />
              <circle cx="12" cy="12" r="1.6" />
              <circle cx="19" cy="12" r="1.6" />
            </svg>
          </button>
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

    <!-- 导入本地书弹窗（自写轻量，无 Element Plus 重组件） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="importOpen" class="dlg-overlay" @click.self="closeImport">
          <div
            ref="dialogRef"
            class="dlg"
            role="dialog"
            aria-modal="true"
            aria-label="导入本地书籍"
            tabindex="-1"
            @keydown.esc="closeImport"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">导入本地书籍</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="uploadBusy" @click="closeImport">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>

            <!-- 虚线拖拽区：点击选择 / 拖入文件 -->
            <div
              class="dropzone"
              :class="{ over: isDragOver, busy: uploadBusy }"
              @click="!uploadBusy && fileInput?.click()"
              @dragover="onDragOver"
              @dragleave="onDragLeave"
              @drop="onDrop"
            >
              <svg class="dz-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 16V4" />
                <path d="M7 9l5-5 5 5" />
                <path d="M4 16v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3" />
              </svg>
              <p class="dz-text">点击选择文件，或将文件拖拽到此处</p>
              <p class="dz-sub">支持 .epub / .txt · 可多选</p>
              <input
                ref="fileInput"
                class="file-input"
                type="file"
                accept=".epub,.txt,application/epub+zip,text/plain"
                multiple
                @change="onPick"
              />
            </div>
            <p v-if="acceptTip" class="accept-tip">{{ acceptTip }}</p>

            <!-- 文件列表：逐个状态 + 细字进度 -->
            <ul v-if="importItems.length" class="file-list">
              <li v-for="(item, i) in importItems" :key="`${item.file.name}-${i}`" class="file-row">
                <span class="file-name" :title="item.file.name">{{ item.file.name }}</span>
                <span class="file-size">{{ fmtSize(item.file.size) }}</span>
                <span class="file-state" :class="item.status">
                  <template v-if="item.status === 'pending'">待导入</template>
                  <template v-else-if="item.status === 'uploading'">
                    <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                      <path d="M21 12a9 9 0 1 1-6.2-8.56" />
                    </svg>
                    {{ item.progress }}%
                  </template>
                  <svg v-else-if="item.status === 'done'" class="state-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4.5 12.5l5 5L19.5 7" />
                  </svg>
                  <template v-else>{{ item.error || '导入失败' }}</template>
                </span>
                <button
                  v-if="item.status === 'pending' && !uploadBusy"
                  class="file-remove"
                  type="button"
                  title="移除"
                  @click="removeItem(i)"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                    <path d="M6 6l12 12M18 6L6 18" />
                  </svg>
                </button>
              </li>
            </ul>

            <!-- 底部：整体进度 / 摘要 + 操作 -->
            <div class="dlg-foot">
              <div v-if="uploadBusy" class="overall">
                <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                  <path d="M21 12a9 9 0 1 1-6.2-8.56" />
                </svg>
                <span>正在导入 {{ uploadIndex + 1 }} / {{ importItems.length }} · {{ totalProgress }}%</span>
              </div>
              <div v-else-if="importDone" class="overall" :class="{ hasError: failedCount > 0 }">
                {{ importSummary }}
              </div>
              <div v-else class="overall"></div>

              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="uploadBusy" @click="closeImport">取消</button>
                <button
                  class="accent-btn"
                  type="button"
                  :disabled="uploadBusy || !hasPending"
                  @click="startUpload"
                >
                  {{ uploadBusy ? '导入中…' : hasPending ? `开始导入（${hasPendingCount}）` : '开始导入' }}
                </button>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 书卡菜单（右键 / 长按 / hover ⋯） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="menuOpen && menuBook" class="ctx-overlay" @click="onOverlayClick" @contextmenu.prevent="closeMenu">
          <div class="ctx-menu" :style="{ left: menuPos.x + 'px', top: menuPos.y + 'px' }" @click.stop>
            <template v-if="!movePanel">
              <button class="ctx-item" type="button" @click="movePanel = true">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 5.5A1.5 1.5 0 0 1 5.5 4h4L12 6.5h6.5A1.5 1.5 0 0 1 20 8v10.5a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5z" />
                </svg>
                移动到分组
              </button>
              <button class="ctx-item danger" type="button" :disabled="menuBusy" @click="removeFromShelf">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 7h16" />
                  <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                  <path d="M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4l.8-12" />
                </svg>
                移出书架
              </button>
            </template>
            <template v-else>
              <button class="ctx-item" type="button" @click="movePanel = false">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M15 6l-6 6 6 6" />
                </svg>
                返回
              </button>
              <div class="ctx-title">移动到分组</div>
              <button class="ctx-item" type="button" :disabled="menuBusy" @click="moveToGroup(0)">未分组</button>
              <button
                v-for="g in groups"
                :key="g.id"
                class="ctx-item"
                type="button"
                :disabled="menuBusy"
                @click="moveToGroup(g.id)"
              >
                {{ g.name }}
              </button>
            </template>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 分组管理弹窗（极简：新建 + 列表 + 删除提示） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="groupOpen" class="dlg-overlay" @click.self="closeGroups">
          <div
            ref="groupDialogRef"
            class="dlg"
            role="dialog"
            aria-modal="true"
            aria-label="分组管理"
            tabindex="-1"
            @keydown.esc="closeGroups"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">分组管理</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="groupSaving" @click="closeGroups">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>

            <!-- 新建分组 -->
            <div class="group-create">
              <input
                v-model="newGroupName"
                class="group-input"
                type="text"
                placeholder="新分组名称"
                maxlength="20"
                spellcheck="false"
                @keydown.enter="createGroup"
              />
              <button
                class="accent-btn"
                type="button"
                :disabled="groupSaving || !newGroupName.trim()"
                @click="createGroup"
              >
                {{ groupSaving ? '创建中…' : '新建' }}
              </button>
            </div>

            <!-- 分组列表：名称 + 本书数 + 删除 -->
            <ul v-if="groups.length" class="group-list">
              <li v-for="g in groups" :key="g.id" class="group-row">
                <span class="group-row-name" :title="g.name">{{ g.name }}</span>
                <span class="group-row-count">{{ groupCount(g.id) }} 本</span>
                <button class="group-del" type="button" title="删除分组" @click="deleteGroupHint(g)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7h16" />
                    <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                    <path d="M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4l.8-12" />
                  </svg>
                </button>
              </li>
            </ul>
            <p v-else class="group-empty">还没有分组，输入名称新建一个吧</p>

            <div class="dlg-foot">
              <span class="overall">删除分组接口后端暂未提供（TODO）</span>
              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="groupSaving" @click="closeGroups">关闭</button>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
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
.nav-link {
  padding: 5px 2px;
  border: none;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.nav-link:hover {
  color: var(--accent);
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

/* 导入本地书按钮（细字描边，hover 加深） */
.import-btn {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 7px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: none;
  color: var(--accent);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.import-btn:hover {
  color: var(--accent-deep);
  border-color: var(--accent-deep);
  background: var(--accent-soft);
}
.import-btn svg {
  width: 13px;
  height: 13px;
}

.refresh-btn {
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
  position: relative;
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

/* ================= 导入本地书弹窗（自写轻量） ================= */
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
  width: min(460px, 100%);
  max-height: calc(100vh - 64px);
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

/* 虚线拖拽区：hover 变强调色 */
.dropzone {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  padding: 34px 16px;
  border: 1.5px dashed var(--border-strong);
  border-radius: var(--radius);
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.dropzone:hover,
.dropzone.over {
  border-color: var(--accent);
  background: var(--accent-soft);
}
.dropzone.busy {
  cursor: default;
  opacity: 0.6;
}
.dz-icon {
  width: 26px;
  height: 26px;
  color: var(--text-3);
  transition: color 0.2s ease;
}
.dropzone:hover .dz-icon,
.dropzone.over .dz-icon {
  color: var(--accent);
}
.dz-text {
  margin: 8px 0 0;
  font-size: 13.5px;
  font-weight: 400;
  color: var(--text-2);
}
.dz-sub {
  margin: 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
}
.file-input {
  display: none;
}
.accept-tip {
  margin: 10px 2px 0;
  font-size: 12px;
  font-weight: 300;
  color: #cf4444;
}

/* 文件列表 */
.file-list {
  list-style: none;
  margin: 14px 0 0;
  padding: 0;
  max-height: 200px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.file-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--bg);
}
.file-name {
  flex: 1;
  min-width: 0;
  font-size: 12.5px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-size {
  flex-shrink: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.file-state {
  flex-shrink: 0;
  min-width: 52px;
  display: inline-flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.file-state.uploading {
  color: var(--accent);
  font-weight: 400;
}
.file-state.done {
  color: #529b2e;
}
.file-state.error {
  color: #cf4444;
  min-width: 0;
  max-width: 130px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.state-icon {
  width: 12px;
  height: 12px;
}
.mini-spin {
  width: 12px;
  height: 12px;
  animation: spin 0.8s linear infinite;
}
.file-remove {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 4px;
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition: color 0.2s ease;
}
.file-remove:hover {
  color: #cf4444;
}
.file-remove svg {
  width: 10px;
  height: 10px;
}

/* 底部：进度 / 摘要 + 操作 */
.dlg-foot {
  margin-top: 16px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.overall {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-2);
}
.overall.hasError {
  color: #cf4444;
}
.dlg-actions {
  display: flex;
  gap: 8px;
  margin-left: auto;
}
.ghost-btn {
  padding: 7px 16px;
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
.ghost-btn:hover:not(:disabled) {
  color: var(--text-1);
  border-color: var(--border-strong);
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

/* 弹窗动画：fade 200ms（遮罩 + 面板轻微上移） */
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

/* ================= 分组栏（胶囊筛选：细字 + 强调色下划线） ================= */
.group-bar {
  display: flex;
  align-items: center;
  gap: 16px;
  margin: -22px 0 32px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  scrollbar-width: none;
}
.group-bar::-webkit-scrollbar {
  display: none;
}
.group-tabs {
  display: flex;
  align-items: center;
  gap: 22px;
  flex: 1;
  min-width: 0;
}
.group-tab {
  position: relative;
  flex-shrink: 0;
  padding: 4px 2px 8px;
  border: none;
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.group-tab:hover {
  color: var(--text-2);
}
.group-tab.active {
  color: var(--accent);
  font-weight: 400;
}
.group-tab.active::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  border-radius: 2px;
  background: var(--accent);
}
.group-manage {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 10px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.group-manage:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.group-manage svg {
  width: 12px;
  height: 12px;
}

/* 书卡右上角 ⋯（hover 显现） */
.card-menu-btn {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 2;
  width: 26px;
  height: 26px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.92);
  color: var(--text-2);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.12);
  cursor: pointer;
  opacity: 0;
  transition:
    opacity 0.2s ease,
    color 0.2s ease;
}
.book-card:hover .card-menu-btn,
.card-menu-btn:focus-visible {
  opacity: 1;
}
.card-menu-btn:hover {
  color: var(--accent);
}
.card-menu-btn svg {
  width: 13px;
  height: 13px;
}

/* ================= 书卡菜单（右键 / 长按 / ⋯） ================= */
.ctx-overlay {
  position: fixed;
  inset: 0;
  z-index: 120;
}
.ctx-menu {
  position: fixed;
  z-index: 121;
  min-width: 168px;
  max-width: 220px;
  max-height: 320px;
  overflow-y: auto;
  padding: 6px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.1);
}
.ctx-item {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  text-align: left;
  cursor: pointer;
  transition:
    color 0.15s ease,
    background-color 0.15s ease;
}
.ctx-item:hover:not(:disabled) {
  color: var(--text-1);
  background: var(--hover);
}
.ctx-item:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.ctx-item.danger {
  color: #cf4444;
}
.ctx-item.danger:hover:not(:disabled) {
  color: #b33535;
  background: rgba(207, 68, 68, 0.07);
}
.ctx-item svg {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
}
.ctx-title {
  padding: 4px 10px 8px;
  font-size: 11px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}

/* ================= 分组管理弹窗 ================= */
.group-create {
  display: flex;
  gap: 8px;
  margin-bottom: 14px;
}
.group-input {
  flex: 1;
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
.group-input::placeholder {
  color: var(--text-3);
  font-weight: 300;
}
.group-input:focus {
  border-color: var(--accent);
}
.group-list {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 260px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.group-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--bg);
}
.group-row-name {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.group-row-count {
  flex-shrink: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.group-del {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 4px;
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease;
}
.group-del:hover {
  color: #cf4444;
  background: rgba(207, 68, 68, 0.08);
}
.group-del svg {
  width: 12px;
  height: 12px;
}
.group-empty {
  margin: 0;
  padding: 28px 0;
  text-align: center;
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .dlg-overlay {
    padding: 16px;
  }
  .dlg {
    max-height: calc(100vh - 32px);
  }
  .import-btn {
    padding: 6px 12px;
    font-size: 12.5px;
  }
  .import-btn span {
    display: none;
  }
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
