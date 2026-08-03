<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { useUserStore } from '@/stores/user'
import { listFiles, downloadFile, uploadFile, mkdir, deleteFile } from '@/api/file'
import type { FileItem } from '@/types'

const router = useRouter()
const store = useUserStore()

/** home 枚举（legacy 对齐）：书仓 / 用户数据 / WebDAV / 空=用户根 */
const HOME_OPTIONS: { label: string; value: string }[] = [
  { label: '书仓', value: '__LOCAL_STORE__' },
  { label: '用户数据', value: '__HOME__' },
  { label: 'WebDAV', value: '__WEBDAV__' },
  { label: '根', value: '' },
]

const home = ref('__LOCAL_STORE__')
const path = ref('')
const files = ref<FileItem[]>([])
const loading = ref(false)
const selectedPath = ref<string | null>(null)

/* ---------------- 弹窗状态 ---------------- */
const uploadOpen = ref(false)
const pickedFile = ref<File | null>(null)
const uploading = ref(false)
const mkdirOpen = ref(false)
const folderName = ref('')

const pickedName = computed(() => pickedFile.value?.name || '')

/* ---------------- 路径工具 ---------------- */
function joinPath(parent: string, name: string): string {
  if (!parent) return name
  return parent.replace(/\/+$/, '') + '/' + name
}

const crumbs = computed(() =>
  path.value
    .split('/')
    .filter(Boolean)
    .map((seg, i, arr) => ({ name: seg, full: arr.slice(0, i + 1).join('/'), last: i === arr.length - 1 })),
)

/* ---------------- 列表 ---------------- */
async function loadList() {
  loading.value = true
  try {
    const res = await listFiles(path.value, home.value)
    const list = (res.data || []) as FileItem[]
    // 目录在前，其余按名称排序
    files.value = [
      ...list.filter((f) => f.isDirectory).sort((a, b) => a.name.localeCompare(b.name)),
      ...list.filter((f) => !f.isDirectory).sort((a, b) => a.name.localeCompare(b.name)),
    ]
  } catch {
    files.value = []
  } finally {
    loading.value = false
  }
}

function switchHome(value: string) {
  if (value === home.value) return
  home.value = value
  path.value = ''
  selectedPath.value = null
  void loadList()
}

function goCrumb(index: number) {
  path.value = index < 0 ? '' : crumbs.value[index].full
  selectedPath.value = null
  void loadList()
}

function enter(item: FileItem) {
  if (item.isDirectory) {
    path.value = joinPath(path.value, item.name)
    selectedPath.value = null
    void loadList()
  } else {
    void download(item)
  }
}

function toggleSelect(item: FileItem) {
  selectedPath.value = selectedPath.value === item.path ? null : item.path
}

/* ---------------- 下载 ---------------- */
async function download(item: FileItem) {
  try {
    const blob = await downloadFile(item.path)
    // legacy 错误体也是 HTTP 200 的 JSON，blob 模式下手动识别
    if (blob.type.includes('application/json')) {
      try {
        const parsed = JSON.parse(await blob.text()) as { errorMsg?: string }
        if (parsed?.errorMsg) ElMessage.error(parsed.errorMsg)
      } catch {
        ElMessage.error('下载失败')
      }
      return
    }
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = item.name
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
  } catch {
    // 请求层已提示
  }
}

/* ---------------- 上传 ---------------- */
function openUpload() {
  pickedFile.value = null
  uploadOpen.value = true
}

function onPick(e: Event) {
  const input = e.target as HTMLInputElement
  pickedFile.value = input.files?.[0] ?? null
}

async function doUpload() {
  if (!pickedFile.value) {
    ElMessage.warning('请先选择文件')
    return
  }
  uploading.value = true
  try {
    await uploadFile(pickedFile.value, path.value)
    ElMessage.success('上传成功')
    uploadOpen.value = false
    pickedFile.value = null
    await loadList()
  } catch {
    // 请求层已提示
  } finally {
    uploading.value = false
  }
}

/* ---------------- 新建文件夹 ---------------- */
async function doMkdir() {
  const name = folderName.value.trim()
  if (!name) {
    ElMessage.warning('请输入文件夹名称')
    return
  }
  try {
    await mkdir(joinPath(path.value, name))
    ElMessage.success('创建成功')
    mkdirOpen.value = false
    folderName.value = ''
    await loadList()
  } catch {
    // 请求层已提示
  }
}

/* ---------------- 删除 ---------------- */
async function removeSelected() {
  const target = files.value.find((f) => f.path === selectedPath.value)
  if (!target) return
  try {
    await ElMessageBox.confirm(
      `确定删除「${target.name}」吗？${target.isDirectory ? '目录及其内容将一并删除。' : ''}`,
      '删除',
      { confirmButtonText: '删除', cancelButtonText: '取消', type: 'warning' },
    )
  } catch {
    return // 用户取消
  }
  try {
    await deleteFile(target.path)
    ElMessage.success('已删除')
    selectedPath.value = null
    await loadList()
  } catch {
    // 请求层已提示
  }
}

/* ---------------- 展示格式化 ---------------- */
function formatSize(n: number | undefined): string {
  if (n == null || n < 0) return '—'
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`
}

function formatTime(v: number | string | undefined): string {
  if (v == null || v === '') return '—'
  if (typeof v === 'number') {
    const d = new Date(v)
    if (Number.isNaN(d.getTime())) return '—'
    const p = (n: number) => String(n).padStart(2, '0')
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
  }
  return String(v)
}

onMounted(() => {
  void loadList()
})
</script>

<template>
  <div class="file-page">
    <!-- 极简导航：字标 + 页面入口 -->
    <header class="topbar">
      <div class="brand">
        <LogoMark class="brand-logo" />
        <span class="brand-name">夜读<span class="brand-dot">.</span></span>
      </div>

      <div class="user-area">
        <button class="nav-link" type="button" @click="router.push('/')">书架</button>
        <button class="nav-link" type="button" @click="router.push('/search')">搜索</button>
        <button class="nav-link" type="button" @click="router.push('/sources')">书源</button>
        <button class="nav-link active" type="button" @click="router.push('/files')">文件</button>
        <span class="user-chip">{{ store.username || '未登录' }}</span>
      </div>
    </header>

    <main class="content">
      <div class="section-head">
        <h1 class="section-title">文件</h1>
        <span class="count">{{ loading ? '…' : `${files.length} 项` }}</span>
      </div>

      <!-- home 切换胶囊 -->
      <div class="home-pills">
        <button
          v-for="opt in HOME_OPTIONS"
          :key="opt.value"
          class="pill"
          :class="{ active: home === opt.value }"
          type="button"
          @click="switchHome(opt.value)"
        >
          {{ opt.label }}
        </button>
      </div>

      <!-- 面包屑 + 工具栏 -->
      <div class="file-bar">
        <nav class="crumbs">
          <button class="crumb" :class="{ current: !path }" type="button" @click="goCrumb(-1)">
            根
          </button>
          <template v-for="(c, i) in crumbs" :key="c.full">
            <span class="crumb-sep">/</span>
            <button class="crumb" :class="{ current: c.last }" type="button" @click="goCrumb(i)">
              {{ c.name }}
            </button>
          </template>
        </nav>

        <div class="toolbar">
          <button class="tool-btn" type="button" @click="openUpload">上传</button>
          <button class="tool-btn" type="button" @click="mkdirOpen = true">新建文件夹</button>
          <button
            class="tool-btn danger"
            type="button"
            :disabled="!selectedPath"
            @click="removeSelected"
          >
            删除
          </button>
        </div>
      </div>

      <!-- 文件列表 -->
      <div class="file-list">
        <div v-if="loading" class="list-hint">加载中…</div>
        <div v-else-if="files.length === 0" class="list-hint empty">此目录为空</div>
        <template v-else>
          <div
            v-for="item in files"
            :key="item.path"
            class="row"
            :class="{ selected: selectedPath === item.path }"
          >
            <button
              class="row-select"
              type="button"
              :title="selectedPath === item.path ? '取消选中' : '选中'"
              @click="toggleSelect(item)"
            >
              <span class="select-dot" />
            </button>
            <button class="row-main" type="button" @click="enter(item)">
              <svg
                v-if="item.isDirectory"
                class="row-icon dir"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
              </svg>
              <svg
                v-else
                class="row-icon file"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.6"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
                <path d="M14 3v5h5" />
              </svg>
              <span class="row-name">{{ item.name }}</span>
            </button>
            <span class="row-size">{{ item.isDirectory ? '—' : formatSize(item.size) }}</span>
            <span class="row-time">{{ formatTime(item.lastModified) }}</span>
          </div>
        </template>
      </div>
    </main>

    <!-- 上传弹窗 -->
    <div v-if="uploadOpen" class="dlg-overlay" @click.self="uploadOpen = false">
      <div class="dlg">
        <h3 class="dlg-title">上传文件</h3>
        <p class="dlg-path">目标目录：{{ path || '根目录' }}</p>
        <label class="file-pick" :class="{ picked: pickedFile }">
          <input type="file" @change="onPick" />
          <span>{{ pickedName || '选择文件' }}</span>
        </label>
        <div class="dlg-actions">
          <button class="btn-plain" type="button" @click="uploadOpen = false">取消</button>
          <button
            class="btn-primary"
            type="button"
            :disabled="!pickedFile || uploading"
            @click="doUpload"
          >
            {{ uploading ? '上传中…' : '上传' }}
          </button>
        </div>
      </div>
    </div>

    <!-- 新建文件夹弹窗 -->
    <div v-if="mkdirOpen" class="dlg-overlay" @click.self="mkdirOpen = false">
      <div class="dlg">
        <h3 class="dlg-title">新建文件夹</h3>
        <p class="dlg-path">位置：{{ path || '根目录' }}</p>
        <input
          v-model="folderName"
          class="dlg-input"
          type="text"
          placeholder="文件夹名称"
          spellcheck="false"
          @keyup.enter="doMkdir"
        />
        <div class="dlg-actions">
          <button class="btn-plain" type="button" @click="mkdirOpen = false">取消</button>
          <button
            class="btn-primary"
            type="button"
            :disabled="!folderName.trim()"
            @click="doMkdir"
          >
            创建
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.file-page {
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
.user-area {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-left: auto;
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
.nav-link:hover,
.nav-link.active {
  color: var(--accent);
}
.user-chip {
  font-size: 13px;
  font-weight: 400;
  color: var(--text-2);
}

/* ================= 内容区 ================= */
.content {
  width: min(760px, 100%);
  margin: 0 auto;
  padding: 40px 32px 72px;
}
.section-head {
  display: flex;
  align-items: baseline;
  gap: 14px;
  margin-bottom: 24px;
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

/* ================= home 切换胶囊 ================= */
.home-pills {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 18px;
}
.pill {
  padding: 6px 16px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease,
    border-color 0.2s ease;
}
.pill:hover {
  color: var(--accent);
  border-color: var(--border-strong);
}
.pill.active {
  color: var(--accent);
  background: var(--accent-soft);
  border-color: var(--accent);
}

/* ================= 面包屑 + 工具栏 ================= */
.file-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 14px;
}
.crumbs {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  flex: 1;
  overflow-x: auto;
  scrollbar-width: none;
}
.crumbs::-webkit-scrollbar {
  display: none;
}
.crumb {
  flex-shrink: 0;
  padding: 4px 2px;
  border: none;
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 0.5px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.crumb:hover {
  color: var(--accent);
}
.crumb.current {
  color: var(--text-1);
  font-weight: 400;
}
.crumb-sep {
  flex-shrink: 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
}
.toolbar {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.tool-btn {
  padding: 5px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    opacity 0.2s ease;
}
.tool-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.tool-btn.danger:hover:not(:disabled) {
  color: #cf4444;
  border-color: #cf4444;
}
.tool-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

/* ================= 文件列表 ================= */
.file-list {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
}
.list-hint {
  padding: 48px 0;
  text-align: center;
  font-size: 13px;
  font-weight: 300;
  color: var(--text-3);
}
.list-hint.empty {
  letter-spacing: 2px;
}
.row {
  display: grid;
  grid-template-columns: 30px 1fr 96px 148px;
  align-items: center;
  gap: 8px;
  padding: 9px 16px;
  border-bottom: 1px solid var(--border);
  transition: background-color 0.2s ease;
}
.row:last-child {
  border-bottom: none;
}
.row:hover {
  background: var(--hover);
}
.row.selected {
  background: var(--accent-soft);
}
.row-select {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: none;
  background: none;
  cursor: pointer;
}
.select-dot {
  width: 15px;
  height: 15px;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  transition:
    background-color 0.2s ease,
    border-color 0.2s ease;
}
.row-select:hover .select-dot {
  border-color: var(--accent);
}
.row.selected .select-dot {
  background: var(--accent);
  border-color: var(--accent);
}
.row-main {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  padding: 2px 0;
  border: none;
  background: none;
  font-family: inherit;
  cursor: pointer;
  text-align: left;
}
.row-icon {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
}
.row-icon.dir {
  color: var(--accent);
}
.row-icon.file {
  color: var(--text-3);
}
.row-name {
  min-width: 0;
  font-size: 13.5px;
  font-weight: 300;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: color 0.2s ease;
}
.row-main:hover .row-name {
  color: var(--accent);
}
.row-size,
.row-time {
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
}
.row-time {
  font-variant-numeric: tabular-nums;
}

/* ================= 弹窗 ================= */
.dlg-overlay {
  position: fixed;
  inset: 0;
  z-index: 50;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(24, 24, 27, 0.32);
  animation: fade-in 0.2s ease both;
}
.dlg {
  width: min(360px, 100%);
  padding: 24px 24px 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: 0 12px 40px rgba(24, 24, 27, 0.12);
}
.dlg-title {
  margin: 0 0 6px;
  font-size: 15px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--text-1);
}
.dlg-path {
  margin: 0 0 16px;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.file-pick {
  display: block;
  padding: 12px 14px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius);
  color: var(--text-3);
  font-size: 12.5px;
  font-weight: 300;
  text-align: center;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.file-pick:hover,
.file-pick.picked {
  color: var(--accent);
  border-color: var(--accent);
}
.file-pick input {
  display: none;
}
.dlg-input {
  box-sizing: border-box;
  width: 100%;
  padding: 10px 12px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--bg);
  color: var(--text-1);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  outline: none;
  transition: border-color 0.2s ease;
}
.dlg-input::placeholder {
  color: var(--text-3);
}
.dlg-input:focus {
  border-color: var(--accent);
}
.dlg-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 20px;
}
.btn-plain,
.btn-primary {
  padding: 7px 18px;
  border-radius: var(--radius);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease,
    border-color 0.2s ease,
    opacity 0.2s ease;
}
.btn-plain {
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
}
.btn-plain:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.btn-primary {
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #ffffff;
}
.btn-primary:hover:not(:disabled) {
  background: var(--accent-deep);
  border-color: var(--accent-deep);
}
.btn-primary:disabled {
  opacity: 0.45;
  cursor: default;
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    flex-wrap: wrap;
    gap: 12px;
    padding: 12px 16px;
  }
  .content {
    padding: 28px 16px 56px;
  }
  .home-pills {
    flex-wrap: nowrap;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .home-pills::-webkit-scrollbar {
    display: none;
  }
  .pill {
    flex-shrink: 0;
  }
  .file-bar {
    flex-direction: column;
    align-items: stretch;
  }
  .toolbar {
    justify-content: flex-end;
  }
  .row {
    /* 移动端简化：仅选择点 + 名称 */
    grid-template-columns: 30px 1fr;
    padding: 11px 12px;
  }
  .row-size,
  .row-time {
    display: none;
  }
  .dlg-overlay {
    padding: 16px;
  }
}
</style>
