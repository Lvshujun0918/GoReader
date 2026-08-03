<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { useUserStore } from '@/stores/user'
import { listFiles, getFile, saveFile, downloadFile, uploadFile, mkdir, deleteFile } from '@/api/file'
import { isNeedSecureKey } from '@/api/users'
import { downloadBlob } from '@/utils/download'
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
const uploadProgress = ref(0)
const mkdirOpen = ref(false)
const folderName = ref('')
const previewOpen = ref(false)
const previewItem = ref<FileItem | null>(null)
const previewContent = ref('')
const previewLoading = ref(false)
const renameOpen = ref(false)
const renameBusy = ref(false)
const renameTarget = ref<FileItem | null>(null)
const renameName = ref('')

const pickedName = computed(() => pickedFile.value?.name || '')

/* ---------------- 文本文件识别 ---------------- */
const TEXT_EXTS = new Set([
  'txt', 'json', 'md', 'markdown', 'log', 'ini', 'conf', 'cfg', 'xml', 'html', 'htm',
  'css', 'js', 'mjs', 'ts', 'csv', 'yml', 'yaml', 'toml', 'srt', 'vtt', 'lrc',
  'properties', 'sh', 'bat', 'cmd', 'sql', 'nfo', 'py', 'java', 'c', 'h', 'cpp', 'rs', 'go',
])

function isTextFile(name: string): boolean {
  const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : ''
  return TEXT_EXTS.has(ext)
}

/** 预览大小上限：超过则点击文件直接下载（getFile 整体读入内存） */
const PREVIEW_MAX_SIZE = 5 * 1024 * 1024

/** secure 模式书仓（__LOCAL_STORE__）写/删需管理密码：轻提示（暂未接入 secureKey 输入框） */
function secureWriteHint(err: unknown): void {
  if (isNeedSecureKey(err)) {
    ElMessage.info('书仓写入/删除需管理密码（secure 模式），暂未接入 secureKey 输入（TODO）')
  }
}

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
    return
  }
  // 文本类小文件：预览（GET file/get）；其余：下载
  if (isTextFile(item.name) && (!(typeof item.size === 'number') || item.size <= PREVIEW_MAX_SIZE)) {
    void openPreview(item)
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
    const blob = await downloadFile(item.path, home.value)
    await downloadBlob(blob, item.name)
  } catch {
    // 请求层已提示
  }
}

/* ---------------- 预览（文本类文件，GET file/get） ---------------- */
async function openPreview(item: FileItem) {
  previewItem.value = item
  previewContent.value = ''
  previewOpen.value = true
  previewLoading.value = true
  try {
    const res = await getFile(item.path, home.value)
    previewContent.value = res.data ?? ''
  } catch {
    previewContent.value = ''
  } finally {
    previewLoading.value = false
  }
}

function closePreview() {
  previewOpen.value = false
  previewItem.value = null
  previewContent.value = ''
}

/* ---------------- 重命名（后端无 API：文本文件用 读旧→写新→删旧 组合） ---------------- */
function openRename() {
  const target = files.value.find((f) => f.path === selectedPath.value)
  if (!target) return
  if (target.isDirectory) {
    ElMessage.info('目录重命名暂不支持（后端暂无重命名 API，TODO）')
    return
  }
  if (!isTextFile(target.name)) {
    ElMessage.info('重命名暂仅支持文本文件（后端无重命名 API，二进制文件为 TODO）')
    return
  }
  renameTarget.value = target
  renameName.value = target.name
  renameOpen.value = true
}

async function doRename() {
  const target = renameTarget.value
  if (!target || renameBusy.value) return
  const name = renameName.value.trim()
  if (!name) {
    ElMessage.warning('请输入新名称')
    return
  }
  if (name.includes('/') || name.includes('\\')) {
    ElMessage.warning('名称不能包含路径分隔符')
    return
  }
  if (name.startsWith('.')) {
    ElMessage.warning('名称不能以 . 开头')
    return
  }
  const dir = target.path.includes('/') ? target.path.slice(0, target.path.lastIndexOf('/')) : ''
  const newPath = joinPath(dir, name)
  if (newPath === target.path) {
    renameOpen.value = false
    return
  }
  if (files.value.some((f) => f.path !== target.path && f.path === newPath)) {
    ElMessage.warning('同名文件已存在')
    return
  }
  renameBusy.value = true
  try {
    // 组合实现：读旧内容 → 写新路径 → 删旧文件（写失败则旧文件保留，不删除）
    const res = await getFile(target.path, home.value)
    await saveFile(newPath, res.data ?? '', home.value)
    await deleteFile(target.path, home.value)
    ElMessage.success('重命名成功')
    renameOpen.value = false
    selectedPath.value = null
    await loadList()
  } catch (err) {
    secureWriteHint(err)
  } finally {
    renameBusy.value = false
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
  uploadProgress.value = 0
  try {
    await uploadFile(pickedFile.value, path.value, home.value, (p) => (uploadProgress.value = p))
    ElMessage.success('上传成功')
    uploadOpen.value = false
    pickedFile.value = null
    await loadList()
  } catch (err) {
    secureWriteHint(err)
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
    await mkdir(path.value, name, home.value)
    ElMessage.success('创建成功')
    mkdirOpen.value = false
    folderName.value = ''
    await loadList()
  } catch (err) {
    secureWriteHint(err)
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
    await deleteFile(target.path, home.value)
    ElMessage.success('已删除')
    selectedPath.value = null
    await loadList()
  } catch (err) {
    secureWriteHint(err)
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
            class="tool-btn"
            type="button"
            :disabled="!selectedPath"
            title="文本文件可用"
            @click="openRename"
          >
            重命名
          </button>
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
        <div v-if="uploading" class="upload-progress">
          <div class="upload-progress-wrap">
            <div class="upload-progress-bar" :style="{ width: `${uploadProgress}%` }" />
          </div>
          <span class="upload-progress-text">{{ uploadProgress }}%</span>
        </div>
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

    <!-- 文件预览弹窗（txt 类，GET file/get） -->
    <div v-if="previewOpen" class="dlg-overlay" @click.self="closePreview">
      <div class="dlg preview-dlg">
        <div class="preview-head">
          <h3 class="dlg-title" :title="previewItem?.name">{{ previewItem?.name }}</h3>
          <div class="preview-actions">
            <button
              class="btn-plain"
              type="button"
              :disabled="!previewItem"
              @click="previewItem && download(previewItem)"
            >
              下载
            </button>
            <button class="btn-plain" type="button" @click="closePreview">关闭</button>
          </div>
        </div>
        <div class="preview-body">
          <p v-if="previewLoading" class="list-hint">加载中…</p>
          <pre v-else class="preview-content">{{ previewContent }}</pre>
        </div>
      </div>
    </div>

    <!-- 重命名弹窗（后端无 API：读旧→写新→删旧，仅文本文件） -->
    <div v-if="renameOpen" class="dlg-overlay" @click.self="renameOpen = false">
      <div class="dlg">
        <h3 class="dlg-title">重命名</h3>
        <p class="dlg-path">{{ renameTarget?.path }}</p>
        <input
          v-model="renameName"
          class="dlg-input"
          type="text"
          placeholder="新名称"
          spellcheck="false"
          @keyup.enter="doRename"
        />
        <p class="rename-tip">后端暂无重命名 API：以「读取内容 → 写入新路径 → 删除旧文件」组合实现（仅文本文件）。</p>
        <div class="dlg-actions">
          <button class="btn-plain" type="button" :disabled="renameBusy" @click="renameOpen = false">
            取消
          </button>
          <button
            class="btn-primary"
            type="button"
            :disabled="renameBusy || !renameName.trim()"
            @click="doRename"
          >
            {{ renameBusy ? '重命名中…' : '确定' }}
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

/* 上传进度条 */
.upload-progress {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 12px;
}
.upload-progress-wrap {
  flex: 1;
  height: 4px;
  border-radius: 2px;
  background: var(--border);
  overflow: hidden;
}
.upload-progress-bar {
  height: 100%;
  border-radius: 2px;
  background: var(--accent);
  transition: width 0.15s ease;
}
.upload-progress-text {
  flex-shrink: 0;
  font-size: 11px;
  font-weight: 300;
  font-variant-numeric: tabular-nums;
  color: var(--text-3);
}

/* 预览弹窗 */
.preview-dlg {
  width: min(620px, 100%);
  display: flex;
  flex-direction: column;
  max-height: 82vh;
}
.preview-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}
.preview-head .dlg-title {
  margin: 0;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.preview-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.preview-body {
  min-height: 120px;
  max-height: 56vh;
  overflow: auto;
  padding: 14px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--bg);
}
.preview-body .list-hint {
  padding: 40px 0;
}
.preview-content {
  margin: 0;
  font-family: 'SF Mono', 'JetBrains Mono', Consolas, monospace;
  font-size: 12px;
  font-weight: 300;
  line-height: 1.7;
  color: var(--text-2);
  white-space: pre-wrap;
  word-break: break-all;
}

/* 重命名提示 */
.rename-tip {
  margin: 10px 0 0;
  font-size: 11.5px;
  font-weight: 300;
  line-height: 1.7;
  color: var(--text-3);
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
