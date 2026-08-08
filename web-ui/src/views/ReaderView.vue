<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave, useRoute, useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import { getBookshelf } from '@/api/bookshelf'
import { getBookInfo, getBookToc, getBookContent } from '@/api/books'
import { saveBook } from '@/api/bookshelf'
import { getHttpTtsList } from '@/api/httpTts'
import { get, post } from '@/api/request'
import { loadReplaceRules } from '@/api/replaceRules'
import { getTtsVoices, synthesizeTts, type TtsVoice } from '@/api/tts'
import { applyHan, getHanMode, type HanMode } from '@/utils/chinese'
import { setGlobalHanMode } from '@/utils/hanMode'
import { DAILY_STATS_KEY, accumulateDaily, parseDailyStats } from '@/utils/dailyStats'
import { useUserStore } from '@/stores/user'
import { loadBookConfig, saveBookConfig, clearBookConfig } from '@/utils/bookConfig'
import { requestWakeLock, releaseWakeLock } from '@/utils/wakeLock'
import { parsePageMode, type PageMode } from '@/utils/readerPageMode'
import {
  loadBgMode,
  loadBgImagePath,
  saveBgMode,
  bgImageUrl as bgImageUrlOf,
  type BgMode,
} from '@/utils/readerBg'
import {
  loadCustomTheme,
  saveCustomTheme,
  customThemeVars,
  customThemeIsDark,
  CUSTOM_THEME_DEFAULTS,
  type ReaderCustomTheme,
} from '@/utils/readerTheme'
import type { Book, BookChapter, Bookmark, HttpTts, ReplaceRule } from '@/types'

const route = useRoute()
const router = useRouter()
const store = useUserStore()

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

/* ---------------- 设置读取/持久化小工具 ---------------- */

function loadSetting(key: string, min: number, max: number, fallback: number, step = 1): number {
  const raw = Number(localStorage.getItem(key))
  if (Number.isNaN(raw) || raw < min || raw > max) return fallback
  return Math.round(raw / step) * step
}
function persist(key: string, value: unknown) {
  try {
    localStorage.setItem(key, String(value))
  } catch {
    /* ignore */
  }
}
const round1 = (v: number) => Math.round(v * 10) / 10

const shelfBook = ref<Book | null>(null)
const bookName = ref('')
const chapters = ref<BookChapter[]>([])
const chapterIndex = ref(0)
const content = ref('')
const loading = ref(true)
const loadError = ref(false)
const notFound = ref(false)
const drawerOpen = ref(false)

/* ---------------- 非文本书（legacy BookType：0 文本/1 音频/2 漫画/3 文件/4 视频） ---------------- */

/** 书籍类型：书架书 type 优先；临时书（query.type）兑底 */
const bookType = computed<number>(() => {
  const t = shelfBook.value?.type
  if (typeof t === 'number' && t >= 0 && t <= 4) return t
  const q = Number(route.query.type)
  return Number.isInteger(q) && q >= 0 && q <= 4 ? q : 0
})
const isTextBook = computed(() => bookType.value === 0)
const isAudioBook = computed(() => bookType.value === 1)
const isComicBook = computed(() => bookType.value === 2)
const isFileBook = computed(() => bookType.value === 3)
const isVideoBook = computed(() => bookType.value === 4)
const isNonTextBook = computed(() => bookType.value !== 0)

/* ---------------- 1. 主题（亮/暗/暖/跟随系统/自定义） ---------------- */

type Theme = 'light' | 'warm' | 'green' | 'dark' | 'black' | 'system' | 'custom'
const THEME_KEY = 'reader_theme'
// 主题预设：白 / 米黄 / 绿 / 黑夜（灰黑）/ 黑夜（全黑）
const THEME_ORDER: Theme[] = ['light', 'warm', 'green', 'dark', 'black']
// 主题名（中文固定文案）
const THEME_NAMES: Record<string, string> = {
  light: '白色',
  warm: '米黄',
  green: '绿色',
  dark: '灰黑夜',
  black: '纯黑夜',
  system: '跟随系统',
  custom: '自定义',
}
function themeName(t: string): string {
  return THEME_NAMES[t] ?? t
}
const theme = ref<Theme>('light')
/** 阅读页根元素（主题只作用于阅读页内，与界面主题 html[data-theme] 分离） */
const pageRef = ref<HTMLElement | null>(null)
/** 系统深色偏好（theme=system 时生效；matchMedia 监听切换） */
const systemDark = ref(false)
let mediaQuery: MediaQueryList | null = null
const systemTheme = (): Theme => (systemDark.value ? 'dark' : 'light')
function applyTheme(t: Theme) {
  // 阅读内容主题仅作用于 .reader-page（变量覆盖见 styles/main.css），不影响书架/设置等界面
  // 自定义主题：基座按背景明暗取 dark/light（color-scheme/滚动条），三色变量经内联样式注入
  if (!pageRef.value) return
  if (t === 'custom') {
    pageRef.value.dataset.readerTheme = customThemeIsDark(customTheme.value) ? 'dark' : 'light'
    return
  }
  const real = t === 'system' ? systemTheme() : t
  pageRef.value.dataset.readerTheme = real
}
function onSystemThemeChange(e: MediaQueryListEvent) {
  systemDark.value = e.matches
  if (theme.value === 'system') applyTheme(theme.value)
}
watch(theme, (t) => {
  applyTheme(t)
  saveSetting(THEME_KEY, t)
})

/* ---------------- 1.1 自定义主题（第 5 档——背景色/文字色/强调色弹层；localStorage reader_theme_custom） ---------------- */

const customTheme = ref<ReaderCustomTheme>(loadCustomTheme())
const customOpen = ref(false)
watch(customTheme, (v) => saveCustomTheme(v), { deep: true })
// 自定义主题下改色：基座明暗可能翻转（浅↔深）→ 重刷 data-reader-theme
watch(
  customTheme,
  () => {
    if (theme.value === 'custom') applyTheme('custom')
  },
  { deep: true },
)
/** 主题选择（设置面板 seg）：选中「自定义」时顺带打开颜色弹层 */
function selectTheme(th: Theme) {
  theme.value = th
  if (th === 'custom') customOpen.value = true
}
/** 恢复默认三色（CUSTOM_THEME_DEFAULTS：米白纸色 + 深褐文字 + 棕金强调） */
function resetCustomTheme() {
  customTheme.value = { ...CUSTOM_THEME_DEFAULTS }
}

/* ---------------- GAP 5：纸纹（细噪点 radial-gradient 微纹理叠在阅读页背景上；3 主题通用，暖色主题最佳） ---------------- */

const TEXTURE_KEY = 'reader_texture'
const paperTexture = ref(false)
{
  const raw = localStorage.getItem(TEXTURE_KEY)
  if (raw === '1') paperTexture.value = true
}
watch(paperTexture, (v) => persist(TEXTURE_KEY, v ? '1' : '0'))

/* ---------------- 2. 排版（行距/段距/字重） ---------------- */

const MIN_LINE = 1.5
const MAX_LINE = 2.5
const MIN_PARA = 0.5
const MAX_PARA = 2
const MIN_WEIGHT = 300
const MAX_WEIGHT = 500

const fontSize = ref<number>(18)
const lineHeight = ref(1.9)
/** 正文宽度档位（窄/适中/宽——max-width） */
const WIDTH_OPTIONS = [
  { label: '窄', value: '720px' },
  { label: '适中', value: '900px' },
  { label: '宽', value: '1080px' },
]
const contentWidth = ref(WIDTH_OPTIONS[1].value)
function setWidth(v: string) {
  contentWidth.value = v
  saveSetting('reader_content_width', v)
}
const paraSpacing = ref(1)
const fontWeight = ref(400)
watch(fontSize, (v) => saveSetting(FONT_KEY, v))
watch(lineHeight, (v) => saveSetting('reader_line_height', v))
watch(paraSpacing, (v) => saveSetting('reader_para_spacing', v))
watch(fontWeight, (v) => saveSetting('reader_font_weight', v))

/* ---------------- 2.01 亮度（0.6-1.4 倍，filter: brightness() 作用于 .reader-main；localStorage: reader_brightness） ---------------- */

const MIN_BRIGHT = 0.6
const MAX_BRIGHT = 1.4
const BRIGHT_KEY = 'reader_brightness'
const brightness = ref(1)
{
  const raw = Number(localStorage.getItem(BRIGHT_KEY))
  if (!Number.isNaN(raw) && raw >= MIN_BRIGHT && raw <= MAX_BRIGHT) brightness.value = raw
}
const round2 = (v: number) => Math.round(v * 100) / 100
watch(brightness, (v) => persist(BRIGHT_KEY, round2(v)))
const brightnessOpen = ref(false)

/* ---------------- 2.1 字体（系统/衬线/圆体/黑体） ---------------- */

type FontKind = 'system' | 'song' | 'hei' | 'kai' | 'fangsong' | 'round' | 'lishu' | 'yahei' | 'pingfang' | 'wenkai' | 'hanserif' | 'serif'
const FONT_OPTIONS: { label: string; value: FontKind }[] = [
  { label: '系统', value: 'system' },
  { label: '宋体', value: 'song' },
  { label: '黑体', value: 'hei' },
  { label: '楷体', value: 'kai' },
  { label: '仿宋', value: 'fangsong' },
  { label: '圆体', value: 'round' },
  { label: '隶书', value: 'lishu' },
  { label: '雅黑', value: 'yahei' },
  { label: '苹方', value: 'pingfang' },
  { label: '文楷', value: 'wenkai' },
  { label: '思源宋', value: 'hanserif' },
  { label: '衬线', value: 'serif' },
]
const FONT_STACK: Record<FontKind, string> = {
  system: '',
  song: "'Songti SC', 'SimSun', 'NSimSun', '宋体', 'Noto Serif CJK SC', serif",
  hei: "'PingFang SC', 'HarmonyOS Sans SC', 'Microsoft YaHei', 'Hiragino Sans GB', sans-serif",
  kai: "'Kaiti SC', 'STKaiti', 'KaiTi', '楷体', '楷体_GB2312', serif",
  fangsong: "'FangSong', 'STFangsong', 'FangSong_GB2312', '仿宋', serif",
  round: "'Yuanti SC', 'YouYuan', '幼圆', 'PingFang SC', sans-serif",
  lishu: "'LiSu', 'STLiti', '隶书', serif",
  yahei: "'Microsoft YaHei', '微软雅黑', 'PingFang SC', sans-serif",
  pingfang: "'PingFang SC', 'SF Pro SC', 'HarmonyOS Sans SC', sans-serif",
  wenkai: "'LXGW WenKai', 'Kaiti SC', '楷体', serif",
  hanserif: "'Source Han Serif CN', 'Songti SC', 'SimSun', '宋体', serif",
  serif: "Georgia, 'Songti SC', 'SimSun', 'Noto Serif CJK SC', serif",
}
const fontKind = ref<FontKind>('system')
const fontOpen = ref(false)
const fontLabel = computed(() => FONT_OPTIONS.find((o) => o.value === fontKind.value)?.label ?? '系统')
watch(fontKind, (v) => saveSetting('reader_font_family', v))
// 点击其他区域关闭字体下拉
function onDocClick(e: MouseEvent) {
  if (fontOpen.value && !(e.target as HTMLElement)?.closest('.font-picker')) {
    fontOpen.value = false
  }
}
onMounted(() => document.addEventListener('mousedown', onDocClick))
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocClick))
const fontFamilyStyle = computed(() => FONT_STACK[fontKind.value])

/* ---------------- 2.2 字距 / 首行缩进 / 对齐 ---------------- */

const letterSpacing = ref(0)
watch(letterSpacing, (v) => saveSetting('reader_letter_spacing', v))

const textIndent = ref(true)
watch(textIndent, (v) => saveSetting('reader_text_indent', v ? '1' : '0'))

const textAlign = ref<'left' | 'justify'>('left')
watch(textAlign, (v) => saveSetting('reader_text_align', v))

const settingsOpen = ref(false)
/** 打开设置：快照当前排版/主题设置（「取消」时回滚） */
let settingsSnapshot: Record<string, unknown> | null = null
function openSettings() {
  settingsSnapshot = {
    fontSize: fontSize.value,
    lineHeight: lineHeight.value,
    paraSpacing: paraSpacing.value,
    fontWeight: fontWeight.value,
    fontKind: fontKind.value,
    letterSpacing: letterSpacing.value,
    textIndent: textIndent.value,
    textAlign: textAlign.value,
    pageMode: pageMode.value,
    contentWidth: contentWidth.value,
    theme: theme.value,
    hanMode: hanMode.value,
    brightness: brightness.value,
    autoSpeed: autoSpeed.value,
  }
  settingsOpen.value = true
}
/** 确认：直接关闭（设置即时生效，无需回滚） */
function confirmSettings() {
  settingsSnapshot = null
  settingsOpen.value = false
}
/** 取消：还原打开时快照后关闭 */
function cancelSettings() {
  const s = settingsSnapshot
  settingsSnapshot = null
  if (s) {
    fontSize.value = s.fontSize as number
    lineHeight.value = s.lineHeight as number
    paraSpacing.value = s.paraSpacing as number
    fontWeight.value = s.fontWeight as number
    fontKind.value = s.fontKind as FontKind
    letterSpacing.value = s.letterSpacing as number
    textIndent.value = s.textIndent as boolean
    textAlign.value = s.textAlign as 'left' | 'justify'
    pageMode.value = s.pageMode as PageMode
    contentWidth.value = s.contentWidth as string
    theme.value = s.theme as Theme
    hanMode.value = s.hanMode as HanMode
    brightness.value = s.brightness as number
    autoSpeed.value = s.autoSpeed as number
  }
  settingsOpen.value = false
}

/** 底部工具栏：主题选择弹层 / 更多菜单 */
const themeOpen = ref(false)
const moreOpen = ref(false)

function resetTypography() {
  fontSize.value = 18
  lineHeight.value = 1.9
  paraSpacing.value = 1
  fontWeight.value = 400
  fontKind.value = 'system'
  letterSpacing.value = 0
  textIndent.value = true
  textAlign.value = 'left'
  brightness.value = 1
}

/* ---------------- 3. 翻页模式（滚动 / 上下翻页 / 左右滑动翻章 / 仿真翻页） ---------------- */

const pageMode = ref<PageMode>('scroll')
watch(pageMode, (m) => saveSetting('reader_page_mode', m))

/** 切章方向：1=下一章（新正文自右滑入）/ -1=上一章（自左滑入）；hslide 模式切章过渡动画用 */
const chapterDir = ref<1 | -1>(1)
/** hslide 模式切章过渡动画类（正文重新挂载时播放一次） */
const chapterAnimClass = computed(() =>
  pageMode.value !== 'hslide'
    ? {}
    : chapterDir.value === 1
      ? { 'chapter-slide-in-right': true }
      : { 'chapter-slide-in-left': true },
)
/** 正文样式（分页模式叠加多栏分页：列宽 = 容器宽，一列一页） */
const contentStyle = computed(() => ({
  fontSize: `${fontSize.value}px`,
  lineHeight: `${lineHeight.value}`,
  fontWeight: `${fontWeight.value}`,
  fontFamily: fontFamilyStyle.value || undefined,
  letterSpacing: letterSpacing.value > 0 ? `${letterSpacing.value}px` : undefined,
  textAlign: textAlign.value,
  ...(isPagedMode() && flipColWidth.value > 0
    ? { columnWidth: `${flipColWidth.value}px`, columnGap: `${FLIP_GAP}px`, height: '100%' }
    : {}),
}))

/* ---- 分页翻页输入（slide/hslide/flip 共用）：滚轮/触屏手势/浮动按钮翻页（不滚动正文） ---- */

let slideAcc = 0
let touchStartY = 0
const SLIDE_THRESHOLD = 48
function isInsideOverlay(el: EventTarget | null): boolean {
  return el instanceof HTMLElement && !!el.closest('.drawer-mask, .pop-mask, .sel-bar')
}
function onWheel(e: WheelEvent) {
  if (isInsideOverlay(e.target)) return
  // 分页模式（slide/hslide/flip）：滚轮增量折算为翻页（不滚动正文）
  if (isPagedMode()) {
    e.preventDefault()
    slideAcc += e.deltaY + e.deltaX
    if (slideAcc >= SLIDE_THRESHOLD) {
      slideAcc = 0
      flipPage(1)
    } else if (slideAcc <= -SLIDE_THRESHOLD) {
      slideAcc = 0
      flipPage(-1)
    }
  }
}
function onTouchStart(e: TouchEvent) {
  touchStartY = e.touches[0]?.clientY ?? 0
}
function onTouchMove(e: TouchEvent) {
  if (pageMode.value !== 'slide' || isInsideOverlay(e.target)) return
  e.preventDefault()
}
function onTouchEnd(e: TouchEvent) {
  if (pageMode.value !== 'slide' || isInsideOverlay(e.target)) return
  const y = e.changedTouches[0]?.clientY ?? touchStartY
  const dy = touchStartY - y
  if (dy >= SLIDE_THRESHOLD) flipPage(1)
  else if (dy <= -SLIDE_THRESHOLD) flipPage(-1)
}

/* ---- 仿真翻页（flip）：CSS 多栏分页（列高 = 视口高，一列一页）+ 横向平滑页过渡 ---- */

/** 分页容器（flip 模式下正文横向分页滚动；其余模式为普通包裹层） */
const flipViewRef = ref<HTMLElement | null>(null)
/** 当前列宽（= 容器内容宽，进入 flip/窗口尺寸/内容宽度变化时重测） */
const flipColWidth = ref(0)
/** 当前所在页（0 基；初始 -1 表示尚未分页，用于禁用上一页按钮） */
const flipPageIdx = ref(-1)
/** 列间距（px） */
const FLIP_GAP = 48
const flipStep = () => flipColWidth.value + FLIP_GAP

/** 是否分页模式（flip 仿真 / slide 上下 / hslide 左右）：CSS 多栏按屏分页，正文不滚动 */
function isPagedMode(): boolean {
  return (
    isTextBook.value &&
    (pageMode.value === 'flip' || pageMode.value === 'slide' || pageMode.value === 'hslide')
  )
}

/** 重测分页列宽（容器宽度变化/进入 flip 模式/换章渲染后调用） */
function measureFlipColumns() {
  const v = flipViewRef.value
  if (!v) return
  flipColWidth.value = v.clientWidth
  flipPageIdx.value = 0
}

/** flip 模式当前横向位置（列轴 px；进度存取用） */
function flipScrollLeft(): number {
  return flipViewRef.value?.scrollLeft ?? 0
}

/** 平滑翻到指定页（越界自动钳制；无分页容器时为空操作） */
function flipGo(page: number) {
  const v = flipViewRef.value
  if (!v) return
  const step = flipStep()
  if (step <= 0) return
  const maxPage = Math.max(0, Math.ceil(v.scrollWidth / step) - 1)
  const target = Math.min(maxPage, Math.max(0, page))
  v.scrollTo({ left: target * step, behavior: 'smooth' })
}

/** 翻一页（dir：1=下一页 / -1=上一页） */
function flipPage(dir: 1 | -1) {
  const v = flipViewRef.value
  if (!v) return
  const step = flipStep()
  if (step <= 0) return
  flipGo(Math.round(v.scrollLeft / step) + dir)
}

/** flip 模式滚动同步：页号/进度/定时保存；近尾（最后 1.5 页）加载下一段并重测分页 */
function onFlipScroll() {
  const v = flipViewRef.value
  if (!v) return
  const step = flipStep()
  flipPageIdx.value = step > 0 ? Math.round(v.scrollLeft / step) : 0
  updateScrollFrac()
  hideSelBar()
  window.clearTimeout(saveTimer)
  saveTimer = window.setTimeout(saveProgress, 300)
  if (v.scrollLeft + v.clientWidth >= v.scrollWidth - step * 1.5) {
    if (maybeLoadMoreChunk()) {
      void nextTick(() => measureFlipColumns())
    }
  }
}

/** 窗口尺寸变化：flip 模式重测列宽 + 刷新进度 */
function onResize() {
  if (isPagedMode()) measureFlipColumns()
  updateScrollFrac()
}

watch(pageMode, (mode, old) => {
  const isPaged = (m: PageMode) => m === 'slide' || m === 'hslide' || m === 'flip'
  const needWheel = isPaged(mode)
  const hadWheel = isPaged(old)
  if (needWheel && !hadWheel) {
    window.addEventListener('wheel', onWheel, { passive: false })
    window.addEventListener('touchstart', onTouchStart, { passive: true })
    window.addEventListener('touchmove', onTouchMove, { passive: false })
    window.addEventListener('touchend', onTouchEnd, { passive: true })
  } else if (!needWheel && hadWheel) {
    window.removeEventListener('wheel', onWheel)
    window.removeEventListener('touchstart', onTouchStart)
    window.removeEventListener('touchmove', onTouchMove)
    window.removeEventListener('touchend', onTouchEnd)
  }
  const srcPaged = isPaged(old)
  const dstPaged = isPaged(mode)
  if (!isTextBook.value) return
  if (dstPaged) {
    // 进入/切换分页模式：保留进度（分页→分页=列轴；滚动→分页=纵向 scrollY 换算列）
    const saved = srcPaged ? flipScrollLeft() : window.scrollY
    window.scrollTo(0, 0)
    void nextTick(() => {
      measureFlipColumns()
      const v = flipViewRef.value
      if (!v || saved <= 0) return
      if (srcPaged) {
        v.scrollTo({ left: saved, behavior: 'auto' })
      } else {
        // 滚动→分页：scrollY / 列高 ≈ 目标列
        const h = v.clientHeight || 1
        v.scrollTo({ left: Math.floor(saved / h) * flipStep(), behavior: 'auto' })
      }
    })
  } else if (srcPaged) {
    // 离开分页：列轴位置换算为纵向位置（同为距章首 px，直接沿用）
    window.scrollTo(0, flipScrollLeft())
  }
})

/** 正文内容宽度变化（720/900/1080）→ 分页模式重测列宽 */
watch(contentWidth, () => {
  if (isPagedMode()) void nextTick(() => measureFlipColumns())
})

/* ---- 左右滑动（hslide）：横向滑动手势/浮动按钮 → 章节级翻页（GAP 85 手势复用 + 切章过渡动画） ---- */

let swipeStartX = 0
let swipeStartY = 0
let swipeTracking = false
const SWIPE_THRESHOLD = 60

function onSwipeTouchStart(e: TouchEvent) {
  if (isInsideOverlay(e.target)) return
  const t = e.touches[0]
  if (!t) return
  swipeStartX = t.clientX
  swipeStartY = t.clientY
  swipeTracking = true
}

function onSwipeTouchEnd(e: TouchEvent) {
  if (!swipeTracking) return
  swipeTracking = false
  // 任一弹层/抽屉/划词工具条打开时不翻页/翻章
  if (
    selOpen.value ||
    drawerOpen.value ||
    settingsOpen.value ||
    themeOpen.value ||
    moreOpen.value ||
    ttsPanelOpen.value ||
    bookmarksOpen.value ||
    jumpOpen.value ||
    searchOpen.value ||
    brightnessOpen.value ||
    customOpen.value ||
    imgViewerOpen.value
  ) {
    return
  }
  const t = e.changedTouches[0]
  if (!t) return
  const dx = t.clientX - swipeStartX
  const dy = t.clientY - swipeStartY
  // 横向主导（|dx|>=60 且明显大于 |dy|）→ 仿真翻页翻页 / 其余模式翻章；纵向滚动/划词选择不处理
  if (Math.abs(dx) < SWIPE_THRESHOLD) return
  if (Math.abs(dy) > Math.abs(dx) * 1.2) return
  if ((window.getSelection()?.toString().trim() ?? '') !== '') return
  if (isPagedMode()) {
    if (dx < 0) flipPage(1)
    else flipPage(-1)
    return
  }
  if (dx < 0) nextChapter()
  else prevChapter()
}

/* ---------------- 屏幕常亮（Wake Lock：进入阅读页且页面可见时请求；切后台/离开释放；不支持环境静默跳过） ---------------- */

const WAKE_LOCK_KEY = 'reader_wake_lock'
/** 默认开启；localStorage 存 '0' 关闭 */
const wakeLockEnabled = ref(localStorage.getItem(WAKE_LOCK_KEY) !== '0')
watch(wakeLockEnabled, (v) => {
  persist(WAKE_LOCK_KEY, v ? '1' : '0')
  if (v) {
    if (document.visibilityState === 'visible') void requestWakeLock()
  } else {
    void releaseWakeLock()
  }
})

/** 页面可见性变化：可见 → 重新请求常亮；隐藏 → 释放 */
function onWakeVisibilityChange() {
  if (document.visibilityState === 'visible') {
    if (wakeLockEnabled.value) void requestWakeLock()
  } else {
    void releaseWakeLock()
  }
}

/* ---------------- 4. 进度显示 + 章节跳转 ---------------- */

const scrollFrac = ref(0)
const jumpOpen = ref(false)
const jumpNum = ref('')

function updateScrollFrac() {
  const v = flipViewRef.value
  if (isPagedMode() && v) {
    const max = v.scrollWidth - v.clientWidth
    scrollFrac.value = max > 0 ? Math.min(1, Math.max(0, v.scrollLeft / max)) : 0
    return
  }
  const max = document.documentElement.scrollHeight - window.innerHeight
  scrollFrac.value = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0
}
const progressPct = computed(() => {
  const n = realChapters.value.length
  if (!n) return 0
  // 非文本书：音频/视频按播放进度、漫画按页进度（scrollFrac 对媒体页无意义）
  if (isAudioBook.value || isVideoBook.value) {
    const dur = isAudioBook.value ? audioDuration.value : videoDuration.value
    const cur = isAudioBook.value ? audioCurrent.value : videoCurrent.value
    const frac = dur > 0 ? Math.min(1, Math.max(0, cur / dur)) : 0
    return Math.min(100, Math.max(0, Math.round(((flatIndex.value + frac) / n) * 100)))
  }
  if (isComicBook.value) {
    const total = comicImages.value.length
    const frac = total > 1 ? comicPage.value / (total - 1) : 0
    return Math.min(100, Math.max(0, Math.round(((flatIndex.value + frac) / n) * 100)))
  }
  const raw = ((flatIndex.value + scrollFrac.value) / n) * 100
  return Math.min(100, Math.max(0, Math.round(raw)))
})
function confirmJump() {
  const n = parseInt(jumpNum.value, 10)
  if (Number.isNaN(n) || n < 1 || n > realChapters.value.length) return
  const target = realChapters.value[n - 1]
  const idx = chapters.value.indexOf(target)
  jumpOpen.value = false
  jumpNum.value = ''
  if (idx >= 0) goToChapter(idx)
}

/* ---------------- 4.1 书签（服务端 /reader3/bookmarks） ---------------- */

const bookmarksOpen = ref(false)
const bookmarks = ref<Bookmark[]>([])
const bookmarkLoading = ref(false)
/** 书签跳转待恢复的段落序号（跨章时随 loadContent 消费） */
let restoreParagraphIdx: number | null = null

async function openBookmarks() {
  bookmarksOpen.value = true
  bookmarkLoading.value = true
  try {
    const res = await get<Bookmark[]>('/getBookmarks', { bookUrl: bookUrl.value })
    bookmarks.value = res.data ?? []
  } catch {
    bookmarks.value = []
  } finally {
    bookmarkLoading.value = false
  }
}

/** 当前视口顶部附近的段落序号（书签锚点） */
function topParagraphIndex(): number {
  const paras = document.querySelectorAll<HTMLElement>('.reader-content .reader-para')
  let idx = 0
  paras.forEach((p, i) => {
    if (p.getBoundingClientRect().top <= 80) idx = i
  })
  return idx
}

async function addBookmark() {
  const ch = currentChapter.value
  if (!ch || !bookUrl.value) return
  const paraIdx = topParagraphIndex()
  const anchor = paragraphs.value[paraIdx]?.trim() ?? ''
  const title = anchor.slice(0, 24) || ch.title
  try {
    await post('/saveBookmark', {
      bookUrl: bookUrl.value,
      title,
      paragraphIndex: paraIdx,
      chapterIndex: chapterIndex.value,
    })
    ElMessage.success('已添加书签')
  } catch {
    /* request.ts 已提示 */
  }
}

function chapterTitleAt(idx: number): string {
  const ch = chapters.value[idx]
  return ch ? hanConvert(ch.title) : ''
}

/** 书签跳转：同章直接滚段落；跨章切章后按段落序号恢复 */
function jumpToBookmark(b: Bookmark) {
  bookmarksOpen.value = false
  const ch = chapters.value[b.chapterIndex]
  if (!ch || ch.isVolume) return
  restoreParagraphIdx = b.paragraphIndex
  if (b.chapterIndex === chapterIndex.value) {
    void applyRestoreParagraph()
  } else {
    goToChapter(b.chapterIndex)
  }
}

async function deleteBookmarkItem(b: Bookmark) {
  try {
    await post('/deleteBookmark', { bookUrl: bookUrl.value, title: b.title })
    bookmarks.value = bookmarks.value.filter((x) => x.title !== b.title)
    ElMessage.success('已删除书签')
  } catch {
    /* request.ts 已提示 */
  }
}

function fmtBookmarkTime(ts: number): string {
  if (!ts) return ''
  const d = new Date(ts)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

/** 按段落序号滚动恢复（含图片撑高后的二次校正） */
async function applyRestoreParagraph() {
  if (restoreParagraphIdx == null) return
  // GAP 155：目标段未渲染时先逐块加载（书签跨段跳转）
  await ensureParagraphRendered(restoreParagraphIdx)
  await nextTick()
  await settleFrames()
  scrollToParagraph(restoreParagraphIdx)
  await imagesReady()
  await settleFrames()
  scrollToParagraph(restoreParagraphIdx)
  restoreParagraphIdx = null
  updateScrollFrac()
}
function scrollToParagraph(idx: number) {
  const p = document.querySelectorAll<HTMLElement>('.reader-content .reader-para')[idx]
  if (!p) return
  if (isPagedMode()) {
    // 仿真翻页：滚动到该段所在列（换算为容器滚动坐标）
    const v = flipViewRef.value
    if (!v) return
    const target = p.getBoundingClientRect().left - v.getBoundingClientRect().left + v.scrollLeft
    v.scrollTo({ left: Math.max(0, target), behavior: 'smooth' })
    return
  }
  const top = Math.max(0, p.getBoundingClientRect().top + window.scrollY - 64)
  window.scrollTo(0, top)
}

/* ---------------- 4.2 页内搜索（前端本地：当前章正文关键词 <mark> 高亮 + 上一个/下一个跳转 + 计数） ---------------- */

const searchOpen = ref(false)
const searchKeyword = ref('')
const searchSearched = ref(false)
/** 每段命中数（与 paragraphs 对齐；未参与搜索时为全 0） */
const paraMatchCounts = ref<number[]>([])
/** 当前命中序号（全局 0 基；-1 = 无命中/未定位） */
const curMatch = ref(-1)
/** 命中总数 */
const matchTotal = computed(() => paraMatchCounts.value.reduce((a, b) => a + b, 0))
/** 搜索是否生效（已搜索且有非空关键词 → 正文渲染 <mark> 高亮） */
const searchActive = computed(() => searchSearched.value && searchKeyword.value.trim().length > 0)
/** 跳转后短暂高亮的目标段落 */
const flashParaIdx = ref(-1)
const searchInputRef = ref<HTMLInputElement | null>(null)
let flashTimer: number | undefined
/** 单段命中上限（防超长段死循环） */
const SEARCH_MAX_PER_PARA = 500

function openChapterSearch() {
  searchKeyword.value = ''
  searchSearched.value = false
  paraMatchCounts.value = []
  curMatch.value = -1
  searchOpen.value = true
  void nextTick(() => searchInputRef.value?.focus())
}

/** 关闭页内搜索并清除全部高亮状态 */
function closeChapterSearch() {
  searchOpen.value = false
  searchKeyword.value = ''
  searchSearched.value = false
  paraMatchCounts.value = []
  curMatch.value = -1
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

/** 全文高亮 HTML：对转义后的段落按关键词（大小写不敏感）把所有命中包 <mark> */
function highlightAll(text: string, kw: string): string {
  const esc = escapeHtml(text)
  const escKw = escapeHtml(kw)
  if (!escKw) return esc
  const lower = esc.toLowerCase()
  const kwLower = escKw.toLowerCase()
  const parts: string[] = []
  let from = 0
  let found = 0
  let i = lower.indexOf(kwLower, from)
  while (i !== -1 && found < SEARCH_MAX_PER_PARA) {
    parts.push(esc.slice(from, i), `<mark>${esc.slice(i, i + escKw.length)}</mark>`)
    from = i + escKw.length
    i = lower.indexOf(kwLower, from)
    found++
  }
  parts.push(esc.slice(from))
  return parts.join('')
}

/** 正文段落渲染：搜索生效且有命中 → 返回带 <mark> 的高亮 HTML；否则 null（走纯文本插值） */
function paraDisplayHtml(i: number): string | null {
  if (!searchActive.value || !paraMatchCounts.value[i]) return null
  return highlightAll(paragraphs.value[i], searchKeyword.value.trim())
}

/** 在当前章段落中统计每个关键词命中数（大小写不敏感；正文按简繁/替换规则转换后的可见文本匹配） */
function runChapterSearch() {
  const kw = searchKeyword.value.trim()
  searchSearched.value = true
  paraMatchCounts.value = []
  curMatch.value = -1
  if (!kw) return
  const lower = kw.toLowerCase()
  const counts: number[] = new Array(paragraphs.value.length).fill(0)
  paragraphs.value.forEach((p, i) => {
    if (!p.toLowerCase().includes(lower)) return
    let n = 0
    let from = 0
    while (n < SEARCH_MAX_PER_PARA) {
      const j = p.toLowerCase().indexOf(lower, from)
      if (j === -1) break
      n++
      from = j + kw.length
    }
    counts[i] = n
  })
  paraMatchCounts.value = counts
  const total = counts.reduce((a, b) => a + b, 0)
  if (total > 0) void scrollToMatch(0)
}

/** 全局命中序号 → 所在段下标 + 段内第几个 mark */
function locateMatch(globalIdx: number): { para: number; mark: number } {
  let acc = 0
  for (let i = 0; i < paraMatchCounts.value.length; i++) {
    const c = paraMatchCounts.value[i]
    if (globalIdx < acc + c) return { para: i, mark: globalIdx - acc }
    acc += c
  }
  return { para: -1, mark: -1 }
}

/** 跳转到第 globalIdx 个命中：滚动到对应段内的 <mark>（GAP 155：目标段未渲染时先加载对应块） */
async function scrollToMatch(globalIdx: number) {
  if (globalIdx < 0 || globalIdx >= matchTotal.value) return
  const { para, mark } = locateMatch(globalIdx)
  if (para < 0) return
  curMatch.value = globalIdx
  await ensureParagraphRendered(para)
  scrollToParagraph(para)
  await nextTick()
  const paraEl = document.querySelector(`.reader-content [data-para="${para}"]`)
  const markEl = paraEl?.querySelectorAll('mark')[mark]
  if (markEl instanceof HTMLElement) {
    // 当前命中 mark 加粗描边（其他命中淡黄底）
    paraEl?.querySelectorAll('mark').forEach((m) => m.classList.remove('mark-current'))
    markEl.classList.add('mark-current')
    const top = markEl.getBoundingClientRect().top + window.scrollY - window.innerHeight / 2
    window.scrollTo({ top: Math.max(0, top), behavior: 'smooth' })
  }
  flashParaIdx.value = para
  window.clearTimeout(flashTimer)
  flashTimer = window.setTimeout(() => {
    flashParaIdx.value = -1
  }, 1600)
}

/** 下一个命中（到尾回绕） */
function nextMatch() {
  if (matchTotal.value === 0) return
  const next = curMatch.value + 1 >= matchTotal.value ? 0 : curMatch.value + 1
  void scrollToMatch(next)
}

/** 上一个命中（到头回绕） */
function prevMatch() {
  if (matchTotal.value === 0) return
  const prev = curMatch.value - 1 < 0 ? matchTotal.value - 1 : curMatch.value - 1
  void scrollToMatch(prev)
}

/* ---------------- 5. 简繁转换（legacy chinese.js 移植） ---------------- */

/** 简繁模式（全局：reader_han_mode——auto 默认，检测繁体自动转简体） */
const hanMode = ref<HanMode>(getHanMode())
const hanTrad = ref(hanMode.value === 'trad')
watch(hanMode, (v) => {
  hanTrad.value = v === 'trad'
  saveSetting('reader_han_mode', v)
})
const detectTraditional = (text: string): boolean => detectTraditionalFn(text)
// chinese.ts 的完整转换表检测（更准）
import { detectTraditional as detectTraditionalFn } from '@/utils/chinese'
watch(content, (c) => {
  if (hanMode.value === 'auto') hanTrad.value = detectTraditional(c)
  // 换章/正文变化：页内搜索命中失效 → 清除高亮与计数
  closeChapterSearch()
  // GAP 155：正文变化 → 分段渲染重置为首块
  resetSegments()
})
function toggleHan() {
  // 三态循环：自动 → 简 → 繁 → 自动
  hanMode.value = hanMode.value === 'auto' ? 'simp' : hanMode.value === 'simp' ? 'trad' : 'auto'
  if (hanMode.value === 'auto') hanTrad.value = detectTraditional(content.value)
}
const hanConvert = (text: string) => applyHan(text, hanMode.value)
const hanTargetLabel = computed(() =>
  hanMode.value === 'auto'
    ? '自动'
    : hanMode.value === 'trad'
      ? '简'
      : '繁',
)

/* ---------------- GAP 6：本书设置（per-book 覆盖 12 项——localStorage reader_book_config_{bookUrl}，本书优先于全局） ---------------- */

/** 本书配置（reader_book_config_{bookUrl}，键 = reader_* 键名，值为字符串） */
const bookOverrides = ref<Record<string, string>>({})
/** 设置面板当前 tab：global=全局偏好 / book=本书设置（默认：本书有覆盖则进本书 tab） */
const bookCfgTab = ref<'global' | 'book'>('global')

function persistBookOverrides() {
  if (bookUrl.value) saveBookConfig(bookUrl.value, bookOverrides.value)
}

/** 从 localStorage 装载本书配置并决定初始 tab */
function loadBookOverrides() {
  bookOverrides.value = bookUrl.value ? loadBookConfig(bookUrl.value) : {}
  bookCfgTab.value = Object.keys(bookOverrides.value).length > 0 ? 'book' : 'global'
}

/**
 * 设置持久化路由：本书 tab → 写入本书配置（覆盖全局）；全局 tab → 写全局键
 * 并清除同名本书覆盖（否则覆盖会静默压过全局修改）。
 */
function saveSetting(key: string, value: unknown) {
  if (bookCfgTab.value === 'book' && bookUrl.value) {
    const o = { ...bookOverrides.value }
    o[key] = String(value)
    bookOverrides.value = o
    persistBookOverrides()
    return
  }
  if (key in bookOverrides.value) {
    const o = { ...bookOverrides.value }
    delete o[key]
    bookOverrides.value = o
    persistBookOverrides()
  }
  if (key === 'reader_han_mode') setGlobalHanMode(value as HanMode)
  else persist(key, value)
}

/** 从全局 localStorage 重新装载 12 项设置 ref（初始化 / 恢复全局默认时调用） */
function loadGlobalIntoRefs() {
  const rawTheme = localStorage.getItem(THEME_KEY)
  // 旧值 paper（纸色）→ 迁移为 warm（暖色）
  theme.value =
    rawTheme === 'dark' || rawTheme === 'warm' || rawTheme === 'system' || rawTheme === 'custom'
      ? rawTheme
      : rawTheme === 'paper'
        ? 'warm'
        : 'light'
  fontSize.value = loadSetting(FONT_KEY, MIN_FONT, MAX_FONT, 18)
  lineHeight.value = loadSetting('reader_line_height', MIN_LINE, MAX_LINE, 1.9, 0.1)
  paraSpacing.value = loadSetting('reader_para_spacing', MIN_PARA, MAX_PARA, 1, 0.1)
  fontWeight.value = loadSetting('reader_font_weight', MIN_WEIGHT, MAX_WEIGHT, 400, 50)
  const w = localStorage.getItem('reader_content_width')
  contentWidth.value = w === '720px' || w === '1080px' ? w : '900px'
  const f = localStorage.getItem('reader_font_family')
  fontKind.value = FONT_OPTIONS.some((o) => o.value === f) ? (f as FontKind) : 'system'
  letterSpacing.value = loadSetting('reader_letter_spacing', 0, 2, 0, 0.5)
  textIndent.value = localStorage.getItem('reader_text_indent') !== '0'
  textAlign.value = localStorage.getItem('reader_text_align') === 'justify' ? 'justify' : 'left'
  pageMode.value = parsePageMode(localStorage.getItem('reader_page_mode'))
  const hm = localStorage.getItem('reader_han_mode')
  hanMode.value = hm === 'simp' || hm === 'trad' ? hm : 'auto'
}

/** 本书覆盖项应用到当前 refs（12 项；值形态与全局键一致，逐项校验） */
function applyBookOverridesToRefs() {
  const o = bookOverrides.value
  const get = (k: string) => o[k]
  const t = get(THEME_KEY)
  // 旧值 paper → 迁移为 warm
  if (t === 'light' || t === 'dark' || t === 'warm' || t === 'system' || t === 'custom') theme.value = t
  else if (t === 'paper') theme.value = 'warm'
  const fs = Number(get(FONT_KEY))
  if (fs >= MIN_FONT && fs <= MAX_FONT) fontSize.value = fs
  const lh = Number(get('reader_line_height'))
  if (lh >= MIN_LINE && lh <= MAX_LINE) lineHeight.value = Math.round(lh * 10) / 10
  const ps = Number(get('reader_para_spacing'))
  if (ps >= MIN_PARA && ps <= MAX_PARA) paraSpacing.value = Math.round(ps * 10) / 10
  const fw = Number(get('reader_font_weight'))
  if (fw >= MIN_WEIGHT && fw <= MAX_WEIGHT) fontWeight.value = Math.round(fw / 50) * 50
  const w = get('reader_content_width')
  if (w === '720px' || w === '900px' || w === '1080px') contentWidth.value = w
  const f = get('reader_font_family')
  if (FONT_OPTIONS.some((o2) => o2.value === f)) fontKind.value = f as FontKind
  const lsp = Number(get('reader_letter_spacing'))
  if (lsp >= 0 && lsp <= 2) letterSpacing.value = Math.round(lsp * 2) / 2
  const ti = get('reader_text_indent')
  if (ti === '1') textIndent.value = true
  else if (ti === '0') textIndent.value = false
  const ta = get('reader_text_align')
  if (ta === 'left' || ta === 'justify') textAlign.value = ta
  const pm = get('reader_page_mode')
  pageMode.value = parsePageMode(pm)
  const hm = get('reader_han_mode')
  if (hm === 'auto' || hm === 'simp' || hm === 'trad') hanMode.value = hm
}

/** 恢复全局默认：清除本书配置 → 回退全局偏好 */
async function restoreGlobalDefaults() {
  try {
    await ElMessageBox.confirm(
      '确定清除本书设置吗？本书将恢复使用全局阅读偏好（字体/字号/行距/主题等 12 项）。',
      '恢复全局默认',
      { confirmButtonText: '清除本书设置', cancelButtonText: '取消', type: 'warning' },
    )
  } catch {
    return // 用户取消
  }
  if (bookUrl.value) clearBookConfig(bookUrl.value)
  bookOverrides.value = {}
  bookCfgTab.value = 'global'
  loadGlobalIntoRefs()
  ElMessage.success('已恢复全局默认（本书设置已清除）')
}

// 初始化顺序：全局值装载 → 本书覆盖装载（决定 tab）→ 覆盖应用（watcher 按 tab 路由持久化，同值不回写）
loadGlobalIntoRefs()
loadBookOverrides()
applyBookOverridesToRefs()

/* ---------------- GAP 4：自定义背景图（纯色/纸纹/图片——设置页选择器；图片经 file/download 展示 + 遮罩保证可读） ---------------- */

const bgMode = ref<BgMode>(loadBgMode())
const bgImagePath = ref(loadBgImagePath())
/** 背景图 URL（file/download + accessToken；路径为空返回 ''） */
const bgImageUrl = computed(() => bgImageUrlOf(bgImagePath.value, store.accessToken))
/** 图片背景遮罩（按当前阅读主题取色，保证文字可读；custom 由 bgStyle 按背景明暗动态取 dark/light） */
const BG_OVERLAY: Record<Theme, string> = {
  light: 'rgba(250, 250, 250, 0.86)',
  dark: 'rgba(17, 17, 20, 0.88)',
  warm: 'rgba(247, 240, 230, 0.88)',
  green: 'rgba(233, 241, 227, 0.88)',
  black: 'rgba(0, 0, 0, 0.88)',
  system: 'rgba(250, 250, 250, 0.86)',
  custom: 'rgba(250, 250, 250, 0.86)',
}
/** 图片背景样式：遮罩渐变叠在图上（cover 铺满 + 固定），失败时回退 var(--bg) */
const bgStyle = computed(() => {
  if (bgMode.value !== 'image' || !bgImageUrl.value) return undefined
  let overlay: string
  if (theme.value === 'custom') {
    overlay = customThemeIsDark(customTheme.value) ? BG_OVERLAY.dark : BG_OVERLAY.light
  } else {
    const real = theme.value === 'system' ? systemTheme() : theme.value
    overlay = BG_OVERLAY[real] ?? BG_OVERLAY.light
  }
  return {
    backgroundImage: `linear-gradient(${overlay}, ${overlay}), url("${bgImageUrl.value}")`,
    backgroundSize: 'cover',
    backgroundPosition: 'center',
    backgroundAttachment: 'fixed',
  }
})
/** 阅读页根元素样式：背景图样式 + 自定义主题 CSS 变量（theme=custom 时注入三色及派生变量） */
const pageStyle = computed(() => {
  if (theme.value !== 'custom') return bgStyle.value
  return [bgStyle.value, customThemeVars(customTheme.value)] as Array<Record<string, string> | undefined>
})
/** 纸纹生效：设置页模式=纸纹，或阅读页开关开启（图片模式优先，不叠加纸纹） */
const effectiveTexture = computed(() => bgMode.value !== 'image' && (paperTexture.value || bgMode.value === 'texture'))
/** 纸纹开关：与背景模式联动（设置页选择器同样写这两个键） */
function toggleTexture() {
  paperTexture.value = !paperTexture.value
  if (paperTexture.value) {
    if (bgMode.value === 'color') {
      bgMode.value = 'texture'
      saveBgMode('texture')
    }
  } else if (bgMode.value === 'texture') {
    bgMode.value = 'color'
    saveBgMode('color')
  }
}

/* ---------------- 6. 替换规则（localStorage: reader_replace_rules，见 api/replaceRules.ts 契约注释） ---------------- */

const REPLACE_KEY = 'reader_replace_enabled'
/** 阅读页总开关：是否应用替换规则（默认开；规则页另有每条规则的启用开关） */
const replaceEnabled = ref(true)
{
  const raw = localStorage.getItem(REPLACE_KEY)
  if (raw === '0') replaceEnabled.value = false
}
/** 当前生效的规则（仅 enabled 且 find 非空） */
const replaceRules = ref<ReplaceRule[]>([])

function refreshReplaceRules() {
  replaceRules.value = loadReplaceRules().filter((r) => r.enabled && r.find && r.find.trim().length > 0)
}

/** 逐条 replaceAll（字面替换，非正则）；空 find 已在上层过滤 */
function applyReplace(text: string): string {
  let out = text
  for (const r of replaceRules.value) {
    out = out.split(r.find).join(r.replace ?? '')
  }
  return out
}

watch(replaceEnabled, (v) => {
  persist(REPLACE_KEY, v ? '1' : '0')
  if (v) refreshReplaceRules()
})

/* ---------------- 7. 听书（后端语音合成：POST /reader3/tts → blob → audio 元素播放；Edge/HttpTTS 引擎；本章播完自动下一章） ---------------- */

const TTS_DEFAULT_VOICE = 'zh-CN-XiaoxiaoNeural'
/** 后端单次合成文本上限（internal/service/tts MAX_TEXT_CHARS） */
const TTS_MAX_CHARS = 20000
const TTS_VOICE_KEY = 'reader_tts_voice'
const TTS_RATE_KEY = 'reader_tts_rate'
const TTS_PITCH_KEY = 'reader_tts_pitch'
const TTS_ENGINE_KEY = 'reader_tts_engine'
const TTS_HTTP_URL_KEY = 'reader_tts_http_url'

type TtsState = 'idle' | 'loading' | 'playing' | 'paused'
const ttsState = ref<TtsState>('idle')
const ttsPanelOpen = ref(false)
const ttsVoices = ref<TtsVoice[]>([])
const ttsVoicesLoaded = ref(false)
const ttsHttpList = ref<HttpTts[]>([])
const ttsHttpLoaded = ref(false)
const ttsVoice = ref(TTS_DEFAULT_VOICE)
const ttsRate = ref(1)
const ttsPitch = ref(0)
const ttsEngine = ref<'edge' | 'http'>('edge')
const ttsHttpUrl = ref('')
const ttsAudioRef = ref<HTMLAudioElement | null>(null)
/** 当前播放 blob 的 objectURL（换源/停止时 revoke） */
let ttsObjectUrl = ''
/** 合成请求序号：切章/停止时自增，丢弃过期结果 */
let ttsLoadSeq = 0
/** 本章播完、待连播下一章标记（loadContent 成功后消费） */
let ttsAutoNext = false

{
  const v = localStorage.getItem(TTS_VOICE_KEY)
  if (v) ttsVoice.value = v
  ttsRate.value = round1(loadSetting(TTS_RATE_KEY, 0.5, 2, 1, 0.1))
  ttsPitch.value = loadSetting(TTS_PITCH_KEY, -10, 10, 0)
  const e = localStorage.getItem(TTS_ENGINE_KEY)
  if (e === 'edge' || e === 'http') ttsEngine.value = e
  ttsHttpUrl.value = localStorage.getItem(TTS_HTTP_URL_KEY) ?? ''
}
watch(ttsVoice, (v) => persist(TTS_VOICE_KEY, v))
watch(ttsRate, (v) => persist(TTS_RATE_KEY, v))
watch(ttsPitch, (v) => persist(TTS_PITCH_KEY, v))
watch(ttsEngine, (v) => persist(TTS_ENGINE_KEY, v))
watch(ttsHttpUrl, (v) => persist(TTS_HTTP_URL_KEY, v))

/** 播放中（顶栏按钮高亮） */
const ttsPlaying = computed(() => ttsState.value === 'playing')

/* ---------------- GAP 7：朗读段落高亮（播放中定时取当前段落，.tts-reading 浅背景） ---------------- */

const ttsReadingPara = ref(-1)
let ttsParaTimer: number | undefined
/** 朗读锚点线：视口高度 32% 处（当前段落 = 最后一个顶部越过锚点的段落） */
const TTS_READ_LINE = 0.32

function updateTtsReadingPara() {
  const paras = document.querySelectorAll<HTMLElement>('.reader-content .reader-para')
  const line = window.innerHeight * TTS_READ_LINE
  let idx = -1
  paras.forEach((p, i) => {
    if (p.getBoundingClientRect().top <= line) idx = i
  })
  if (ttsReadingPara.value !== idx) ttsReadingPara.value = idx
}

/** 播放/继续时启动定时器（暂停时保留高亮位置） */
function startTtsParaTracking() {
  stopTtsParaTracking()
  updateTtsReadingPara()
  ttsParaTimer = window.setInterval(updateTtsReadingPara, 400)
}

/** 暂停：仅停定时器，保留当前高亮段落 */
function pauseTtsParaTracking() {
  if (ttsParaTimer !== undefined) {
    window.clearInterval(ttsParaTimer)
    ttsParaTimer = undefined
  }
}

/** 停止/切章：停定时器并清除高亮 */
function stopTtsParaTracking() {
  pauseTtsParaTracking()
  ttsReadingPara.value = -1
}
/** 顶栏按钮文案 */
const ttsTopLabel = computed(() =>
  ttsState.value === 'playing' || ttsState.value === 'loading'
    ? '停止'
    : ttsState.value === 'paused'
      ? '继续'
      : '听书',
)

/** 语速 0.5-2.0 → Edge 百分比（+0% / +10% / -50%） */
const ttsRateParam = computed(() => {
  const pct = Math.round((ttsRate.value - 1) * 100)
  return `${pct >= 0 ? '+' : ''}${pct}%`
})
/** 音调 → Edge Hz（+0Hz / -2Hz） */
const ttsPitchParam = computed(() => `${ttsPitch.value >= 0 ? '+' : ''}${ttsPitch.value}Hz`)

/** 音色下拉按 locale 分组 */
const ttsLocaleGroups = computed(() => {
  const map = new Map<string, TtsVoice[]>()
  for (const v of ttsVoices.value) {
    const arr = map.get(v.locale) ?? []
    arr.push(v)
    map.set(v.locale, arr)
  }
  return Array.from(map.entries()).map(([label, voices]) => ({ label, voices }))
})

/** 首次打开面板时加载语音列表 + HttpTTS 列表（记忆值失效时回退默认） */
async function loadTtsOptions() {
  if (!ttsVoicesLoaded.value) {
    ttsVoicesLoaded.value = true
    try {
      const res = await getTtsVoices()
      ttsVoices.value = res.data ?? []
    } catch {
      ttsVoices.value = []
    }
    if (ttsVoices.value.length > 0 && !ttsVoices.value.some((v) => v.value === ttsVoice.value)) {
      ttsVoice.value = TTS_DEFAULT_VOICE
    }
  }
  if (!ttsHttpLoaded.value) {
    ttsHttpLoaded.value = true
    try {
      const res = await getHttpTtsList()
      ttsHttpList.value = res.data ?? []
    } catch {
      ttsHttpList.value = []
    }
    if (ttsHttpList.value.length > 0) {
      if (!ttsHttpList.value.some((t) => t.url === ttsHttpUrl.value)) {
        ttsHttpUrl.value = ttsHttpList.value[0].url
      }
    } else if (ttsEngine.value === 'http') {
      ttsEngine.value = 'edge'
    }
  }
}
watch(ttsPanelOpen, (open) => {
  if (open) void loadTtsOptions()
})

/** 朗读文本：正文段落（含替换规则/简繁转换），截断到后端上限 */
function ttsText(): string {
  return paragraphs.value.join('。').slice(0, TTS_MAX_CHARS)
}

/** 播放当前章：合成 → blob → audio 播放 */
async function startTts() {
  ttsSelectionMode = false // 整章朗读：结束允许自动连播
  const text = ttsText()
  if (!text) {
    ElMessage.info('本章暂无内容可朗读')
    return
  }
  await loadTtsOptions()
  const audio = ttsAudioRef.value
  if (!audio) return
  if (ttsEngine.value === 'http' && !ttsHttpUrl.value) {
    ElMessage.info('请先在设置页添加 HttpTTS 源')
    return
  }
  const seq = ++ttsLoadSeq
  ttsState.value = 'loading'
  let blob: Blob
  try {
    blob = await synthesizeTts({
      text,
      voice: ttsVoice.value,
      rate: ttsRateParam.value,
      pitch: ttsPitchParam.value,
      engine: ttsEngine.value,
      httpUrl: ttsEngine.value === 'http' ? ttsHttpUrl.value : undefined,
    })
  } catch (e) {
    if (seq === ttsLoadSeq) {
      ttsState.value = 'idle'
      ElMessage.error(e instanceof Error ? e.message : '语音合成失败')
    }
    return
  }
  const url = URL.createObjectURL(blob)
  if (seq !== ttsLoadSeq) {
    // 期间已停止/切章：丢弃过期结果
    URL.revokeObjectURL(url)
    return
  }
  if (ttsObjectUrl) URL.revokeObjectURL(ttsObjectUrl)
  ttsObjectUrl = url
  audio.pause()
  audio.src = url
  ttsState.value = 'playing'
  // 后台标签页被浏览器暂停时，切回自动恢复（防"听书几分钟后中断"）
  document.addEventListener(
    'visibilitychange',
    () => {
      const a = ttsAudioRef.value
      if (document.visibilityState === 'visible' && ttsState.value === 'playing' && a && a.paused && a.src) {
        void a.play().catch(() => {})
      }
    },
    { once: true },
  )
  try {
    await audio.play()
    startTtsParaTracking()
  } catch {
    // 自动播放被拦截（异步 fetch 后手势已失效）：保持待播，面板点「播放」即可恢复
    if (ttsState.value === 'playing') ttsState.value = 'paused'
  }
}

function stopTts() {
  ttsLoadSeq++
  ttsAutoNext = false
  ttsSelectionMode = false
  ttsState.value = 'idle'
  stopTtsParaTracking()
  const audio = ttsAudioRef.value
  if (audio) {
    audio.pause()
    audio.removeAttribute('src')
    audio.load()
  }
  if (ttsObjectUrl) {
    URL.revokeObjectURL(ttsObjectUrl)
    ttsObjectUrl = ''
  }
}

function pauseTts() {
  const audio = ttsAudioRef.value
  if (!audio || ttsState.value !== 'playing') return
  audio.pause()
  ttsState.value = 'paused'
  pauseTtsParaTracking()
}

function resumeTts() {
  const audio = ttsAudioRef.value
  if (!audio || ttsState.value !== 'paused') return
  void audio.play().catch(() => {
    /* 保持暂停 */
  })
  startTtsParaTracking()
}

/** 面板播放/暂停/继续 */
function ttsPlayPause() {
  if (ttsState.value === 'playing') pauseTts()
  else if (ttsState.value === 'paused') resumeTts()
  else if (ttsState.value === 'idle') void startTts()
}

/** 顶栏听书按钮：展开面板 + 播放/继续/停止 */
function toggleTts() {
  ttsPanelOpen.value = true
  if (ttsState.value === 'playing' || ttsState.value === 'loading') stopTts()
  else if (ttsState.value === 'paused') resumeTts()
  else void startTts()
}

/** 本章播完：自动连播下一章；最后一章则停止；划词朗读播完即止 */
function onTtsEnded() {
  if (ttsState.value === 'idle') return
  stopTtsParaTracking()
  if (ttsObjectUrl) {
    URL.revokeObjectURL(ttsObjectUrl)
    ttsObjectUrl = ''
  }
  const audio = ttsAudioRef.value
  if (audio) {
    audio.removeAttribute('src')
    audio.load()
  }
  // GAP 92：划词朗读播完即止（不自动切章）
  if (ttsSelectionMode) {
    ttsSelectionMode = false
    ttsState.value = 'idle'
    return
  }
  if (hasNext.value) {
    ttsAutoNext = true
    ttsState.value = 'idle'
    nextChapter()
  } else {
    ttsState.value = 'idle'
    ElMessage.info('本书已播放完毕')
  }
}

let ttsErrorRetries = 0
function onTtsError() {
  if (ttsState.value === 'idle') return
  const audio = ttsAudioRef.value
  if (audio && !audio.getAttribute('src')) return
  // 自动重试一次（网络/解码抖动），仍失败才停止
  if (ttsErrorRetries < 1 && audio && audio.src) {
    ttsErrorRetries++
    const src = audio.src
    audio.pause()
    audio.load()
    audio.src = src
    audio
      .play()
      .then(() => {
        ttsErrorRetries = 0
      })
      .catch(() => {
        stopTts()
        ElMessage.error('语音播放失败（已重试）')
      })
    return
  }
  ttsErrorRetries = 0
  stopTts()
  ElMessage.error('语音播放失败')
}

/* ---------------- GAP 92：划词朗读（选中文本 TTS——复用现有合成/播放流程，播完不自动切章） ---------------- */

/** 划词朗读模式：本章播完只停止不连播 */
let ttsSelectionMode = false

/** 朗读指定文本（划词朗读入口） */
async function speakText(text: string) {
  const clipped = text.slice(0, TTS_MAX_CHARS)
  if (!clipped.trim()) return
  await loadTtsOptions()
  const audio = ttsAudioRef.value
  if (!audio) return
  if (ttsEngine.value === 'http' && !ttsHttpUrl.value) {
    ElMessage.info('请先在设置页添加 HttpTTS 源')
    return
  }
  const seq = ++ttsLoadSeq
  ttsState.value = 'loading'
  let blob: Blob
  try {
    blob = await synthesizeTts({
      text: clipped,
      voice: ttsVoice.value,
      rate: ttsRateParam.value,
      pitch: ttsPitchParam.value,
      engine: ttsEngine.value,
      httpUrl: ttsEngine.value === 'http' ? ttsHttpUrl.value : undefined,
    })
  } catch (e) {
    if (seq === ttsLoadSeq) {
      ttsState.value = 'idle'
      ElMessage.error(e instanceof Error ? e.message : '语音合成失败')
    }
    return
  }
  const url = URL.createObjectURL(blob)
  if (seq !== ttsLoadSeq) {
    // 期间已停止/切章：丢弃过期结果
    URL.revokeObjectURL(url)
    return
  }
  if (ttsObjectUrl) URL.revokeObjectURL(ttsObjectUrl)
  ttsObjectUrl = url
  audio.pause()
  audio.src = url
  ttsState.value = 'playing'
  ttsSelectionMode = true
  try {
    await audio.play()
  } catch {
    // 自动播放被拦截（异步合成后手势已失效）：保持待播，面板点「播放」即可恢复
    if (ttsState.value === 'playing') ttsState.value = 'paused'
  }
}

/** 划词工具条「朗读」按钮：朗读选中文本 */
function speakSelection() {
  const text = selText.value
  clearSelection()
  hideSelBar()
  if (!text) return
  void speakText(text)
}

/* ---------------- 8. 自动阅读（定时滚动，到底自动切章） ---------------- */

const autoPlaying = ref(false)
const autoSpeed = ref(3)
autoSpeed.value = loadSetting('reader_auto_speed', 1, 5, 3)
watch(autoSpeed, (v) => persist('reader_auto_speed', v))
let autoTimer: number | undefined
const AUTO_BOTTOM_GAP = 40
const autoInterval = () => (6 - autoSpeed.value) * 1000

function autoTick() {
  if (loading.value || loadError.value || !currentChapter.value) return
  // 分页模式（slide/hslide/flip）：自动翻页（不滚动）
  if (isPagedMode()) {
    const v = flipViewRef.value
    if (v) {
      // 已到末页：先尝试加载下一块内容；否则切章/停止
      if (v.scrollLeft + v.clientWidth >= v.scrollWidth - 4) {
        if (maybeLoadMoreChunk()) {
          void nextTick(() => measureFlipColumns())
          return
        }
        if (hasNext.value) nextChapter()
        else stopAuto()
        return
      }
    }
    flipPage(1)
    return
  }
  // 滚动模式：自动滚动
  const doc = document.documentElement
  if (doc.scrollHeight - window.innerHeight <= 0) return
  // GAP 155：长章还有未渲染段且已近底——先加载下一块，不切章
  if (maybeLoadMoreChunk()) return
  const atBottom = window.scrollY + window.innerHeight >= doc.scrollHeight - AUTO_BOTTOM_GAP
  if (atBottom) {
    if (hasNext.value) nextChapter()
    else stopAuto()
    return
  }
  // 每 tick 滚动一行（行高 = 字号 × 行距）
  window.scrollBy({ top: fontSize.value * lineHeight.value, behavior: 'auto' })
}

function startAuto() {
  if (autoTimer != null) return
  autoPlaying.value = true
  autoTick()
  autoTimer = window.setInterval(autoTick, autoInterval())
}
function stopAuto() {
  autoPlaying.value = false
  if (autoTimer != null) {
    window.clearInterval(autoTimer)
    autoTimer = undefined
  }
}
function toggleAuto() {
  if (autoPlaying.value) stopAuto()
  else startAuto()
}
// 运行中调速：重建定时器
watch(autoSpeed, () => {
  if (autoPlaying.value && autoTimer != null) {
    window.clearInterval(autoTimer)
    autoTimer = window.setInterval(autoTick, autoInterval())
  }
})

/* ---------------- 9. 划词操作（复制 / 搜索） ---------------- */

const selText = ref('')
const selOpen = ref(false)
const selX = ref(0)
const selY = ref(0)

function hideSelBar() {
  selOpen.value = false
  selText.value = ''
}

function clearSelection() {
  window.getSelection()?.removeAllRanges()
}

/** mouseup/touchend 后延迟计算：确保选中态已就绪 */
function onSelectionUp() {
  window.setTimeout(computeSelection, 0)
}

function computeSelection() {
  const sel = window.getSelection()
  const text = sel?.toString().trim() ?? ''
  if (!text || !sel || sel.rangeCount === 0) {
    hideSelBar()
    return
  }
  const node = sel.anchorNode
  const el =
    node && node.nodeType === Node.TEXT_NODE ? node.parentElement : (node as HTMLElement | null)
  if (!el || !el.closest('.reader-content')) {
    hideSelBar()
    return
  }
  const rect = sel.getRangeAt(0).getBoundingClientRect()
  if (rect.width === 0 && rect.height === 0) {
    hideSelBar()
    return
  }
  selText.value = text
  selX.value = Math.min(Math.max(rect.left + rect.width / 2, 60), window.innerWidth - 60)
  selY.value = Math.max(10, rect.top - 44)
  selOpen.value = true
}

/** 点击正文/他处收起工具条（mousedown 先于 mouseup，避免选中后立即被 click 误关） */
function onDocMouseDown(e: MouseEvent) {
  if (e.target instanceof HTMLElement && e.target.closest('.sel-bar')) return
  hideSelBar()
}

async function copySelection() {
  const text = selText.value
  clearSelection()
  hideSelBar()
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    try {
      const ta = document.createElement('textarea')
      ta.value = text
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
    } catch {
      /* ignore */
    }
  }
  ElMessage.success('已复制')
}

/* ---------------- GAP 124：复制本章（navigator.clipboard 全文；失败回退 execCommand；提示字数） ---------------- */

async function copyChapter() {
  const text = paragraphs.value.join('\n')
  if (!text) {
    ElMessage.info('本章暂无内容可复制')
    return
  }
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    try {
      const ta = document.createElement('textarea')
      ta.value = text
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
    } catch {
      ElMessage.error('复制失败，请手动长按选择复制')
      return
    }
  }
  ElMessage.success(`已复制本章（${text.length} 字）`)
}

/* ---------------- 10. 章节图片预加载（下一章前 5 张） ---------------- */

const preloadedChapters = new Set<string>()

/** 提取正文图片 URL：markdown 图片语法 + 裸图片扩展名 URL */
function extractImageUrls(text: string): string[] {
  const urls: string[] = []
  const mdRe = /!\[[^\]]*]\(\s*([^)\s]+)\s*\)/g
  let m: RegExpExecArray | null
  while ((m = mdRe.exec(text))) urls.push(m[1])
  const bareRe =
    /https?:\/\/[^\s"'<>，。！？、；：“”‘’（）【】]+\.(?:png|jpe?g|gif|webp|bmp|svg)(?:\?[^\s"'<>]*)?/gi
  let b: RegExpExecArray | null
  while ((b = bareRe.exec(text))) urls.push(b[0])
  return urls
}

/** 当前章渲染后调用：静默拉取下一章正文并预热前 5 张图片（仅当含图片 URL）；文本书专用 */
function preloadNextChapterImages() {
  if (!isTextBook.value) return
  if (!shelfBook.value?.origin) return
  const fi = flatIndex.value
  if (fi < 0 || fi >= realChapters.value.length - 1) return
  const next = realChapters.value[fi + 1]
  if (preloadedChapters.has(next.url)) return
  preloadedChapters.add(next.url)
  void getBookContent(next.url, shelfBook.value.origin)
    .then((res) => {
      for (const u of extractImageUrls(res.data?.content ?? '').slice(0, 5)) {
        const img = new Image()
        img.src = u
      }
    })
    .catch(() => {
      /* 静默 */
    })
}

/* ---------------- GAP 102：正文图片全屏查看（段落为单张图片时渲染 <img>，点击全屏） ---------------- */

const imgViewerOpen = ref(false)
const imgViewerUrl = ref('')

function openImgViewer(url: string) {
  imgViewerUrl.value = url
  imgViewerOpen.value = true
}

function closeImgViewer() {
  imgViewerOpen.value = false
  imgViewerUrl.value = ''
}

/** 段落是否为单张图片（markdown 语法或裸图片 URL）→ 返回图片地址，否则 null */
function singleImageUrl(para: string): string | null {
  const md = /^!\[[^\]]*]\(\s*([^)\s]+)\s*\)$/.exec(para)
  if (md) return md[1]
  if (
    /^https?:\/\/[^\s"'<>，。！？、；：“”‘’（）【】]+\.(?:png|jpe?g|gif|webp|bmp|svg)(?:\?[^\s"'<>]*)?$/i.test(
      para,
    )
  ) {
    return para
  }
  return null
}

/** 与 paragraphs 同源切分（原始文本，不经替换/简繁转换），逐段标注图片地址 */
const paraImgs = computed<(string | null)[]>(() =>
  content.value
    .split(/\n+/)
    .map((s) => s.trim())
    .filter(Boolean)
    .map(singleImageUrl),
)

/* ---------------- 目录/章节 ---------------- */

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
    .filter(Boolean)
    .map((p) => applyReplace(hanConvert(p))),
)

/* ---------------- GAP 155：长章分段渲染（>200 段按 200 段/块渐进渲染——滚动到底加载下一块，防超长章 DOM 过大） ---------------- */

const SEGMENT_SIZE = 200
/** 已渲染段数（resetSegments 初始化 / 滚动到底 +200；始终 ≤ paragraphs.length） */
const renderedCount = ref(0)
const visibleParagraphs = computed(() => paragraphs.value.slice(0, renderedCount.value))
/** 与 visibleParagraphs 对齐的图片标注（GAP 102 单图段落） */
const visibleParaImgs = computed(() => paraImgs.value.slice(0, renderedCount.value))
const hasMoreParagraphs = computed(() => renderedCount.value < paragraphs.value.length)

/** 换章/正文变化时重置为首块 */
function resetSegments() {
  renderedCount.value = Math.min(SEGMENT_SIZE, paragraphs.value.length)
}

/** 滚动近底（600px）时加载下一块；返回是否加载了新块 */
function maybeLoadMoreChunk(): boolean {
  if (!hasMoreParagraphs.value) return false
  if (loading.value || loadError.value) return false
  const doc = document.documentElement
  if (doc.scrollHeight - window.innerHeight - window.scrollY > 600) return false
  renderedCount.value = Math.min(renderedCount.value + SEGMENT_SIZE, paragraphs.value.length)
  return true
}

/** 确保第 idx 段已渲染（书签/搜索跳转/滚动恢复前逐块加载直到覆盖目标） */
async function ensureParagraphRendered(idx: number) {
  while (renderedCount.value <= idx && hasMoreParagraphs.value) {
    renderedCount.value = Math.min(renderedCount.value + SEGMENT_SIZE, paragraphs.value.length)
    await nextTick()
    await settleFrames()
  }
}
/* ---------------- 章节字数（目录抽屉：后端 chapterWordCount 优先；书源书从前端已缓存正文估算，未加载章省略） ---------------- */

/** 当前会话已加载正文的字数：chapterUrl → 字符数（与后端 length(content) 口径一致） */
const chapterWordCounts = ref<Record<string, number>>({})
/** 章节字数：后端字段（本地书）→ 本会话缓存正文估算 → 未知返回 null（目录省略显示） */
function chapterWordCountOf(ch: BookChapter): number | null {
  const wc = typeof ch.chapterWordCount === 'number' ? ch.chapterWordCount : chapterWordCounts.value[ch.url]
  return typeof wc === 'number' && wc > 0 ? wc : null
}
/** 格式化：≥1 万显示「x.x万字」，否则「n字」 */
function fmtWordCount(n: number): string {
  return n >= 10000 ? `${(n / 10000).toFixed(1)}万字` : `${n}字`
}
/** 目录项字数文案（null = 不显示） */
function chapterWordCountLabel(ch: BookChapter): string | null {
  const n = chapterWordCountOf(ch)
  return n === null ? null : fmtWordCount(n)
}

const displayBookName = computed(() => hanConvert(bookName.value))
const displayChapterTitle = computed(() => (currentChapter.value ? hanConvert(currentChapter.value.title) : ''))
/** 目录项（含卷标题）统一按当前简繁模式转换；wcLabel=字数文案（未加载章为 null） */
const drawerChapters = computed(() =>
  chapters.value.map((c) => ({ ...c, title: hanConvert(c.title), wcLabel: chapterWordCountLabel(c) })),
)

/* ---------------- 目录卷折叠（点击卷标题折叠/展开；localStorage reader_toc_collapsed {volTitle: bool}） ---------------- */

const TOC_COLLAPSED_KEY = 'reader_toc_collapsed'
/** 折叠状态：卷标题（简繁转换后展示名）→ 是否折叠 */
const tocCollapsed = ref<Record<string, boolean>>({})
{
  try {
    const raw = JSON.parse(localStorage.getItem(TOC_COLLAPSED_KEY) ?? '{}')
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) tocCollapsed.value = raw
  } catch {
    /* ignore */
  }
}

/** 点击卷标题行：折叠/展开 + 持久化 */
function toggleVolume(title: string) {
  tocCollapsed.value = { ...tocCollapsed.value, [title]: !tocCollapsed.value[title] }
  try {
    localStorage.setItem(TOC_COLLAPSED_KEY, JSON.stringify(tocCollapsed.value))
  } catch {
    /* ignore */
  }
}

/** 与 drawerChapters 对齐：非卷条目是否被所在卷折叠隐藏（卷标题行本身始终显示） */
const chapterHidden = computed(() => {
  const out: boolean[] = []
  let currentCollapsed = false
  for (const c of drawerChapters.value) {
    if (c.isVolume) currentCollapsed = !!tocCollapsed.value[c.title]
    out.push(currentCollapsed)
  }
  return out
})

/** 首次进入需要恢复的滚动位置（正文渲染完成后应用一次） */
let restoreScrollY: number | null = null
let saveTimer: number | undefined

/* ---------------- 进度存取 ---------------- */

function progressKey(): string {
  return `reader-progress-${bookUrl.value}`
}

/** 当前阅读位置（纵向滚动 px；仿真翻页为列轴 px；非文本书为媒体秒数/漫画页索引） */
function currentPos(): number {
  if (isNonTextBook.value) return mediaPosition()
  return isPagedMode() ? flipScrollLeft() : window.scrollY
}

function saveProgress() {
  if (!currentChapter.value) return
  try {
    localStorage.setItem(
      progressKey(),
      JSON.stringify({
        chapterIndex: chapterIndex.value,
        // 非文本书：scrollY 槽位存音频/视频秒数或漫画页索引
        scrollY: currentPos(),
        updatedAt: Date.now(),
      } satisfies ReaderProgress),
    )
  } catch {
    /* ignore */
  }
  syncServerProgress()
}

/* ---------------- GAP 110：每日阅读时长累计（本机——统计弹窗近 7 天柱状图数据源） ---------------- */

/** 累计周期（秒）：每 30s 向当日桶 +30s；仅页面可见且正文就绪时累计 */
const DAILY_TICK_SECONDS = 30
let dailyTimer: number | undefined

function startDailyTracker() {
  stopDailyTracker()
  dailyTimer = window.setInterval(() => {
    if (document.visibilityState !== 'visible') return
    if (loading.value || loadError.value || !currentChapter.value) return
    try {
      const map = parseDailyStats(localStorage.getItem(DAILY_STATS_KEY))
      localStorage.setItem(DAILY_STATS_KEY, JSON.stringify(accumulateDaily(map, DAILY_TICK_SECONDS)))
    } catch {
      /* ignore */
    }
  }, DAILY_TICK_SECONDS * 1000)
}

function stopDailyTracker() {
  if (dailyTimer !== undefined) {
    window.clearInterval(dailyTimer)
    dailyTimer = undefined
  }
}

/** 进度服务端同步（POST /reader3/saveBookProgress；失败静默，不影响本地阅读） */
function syncServerProgress() {
  if (!shelfBook.value || !currentChapter.value) return
  void post('/saveBookProgress', {
    // 用书对象自身的 bookUrl（不依赖 route）——组件卸载时路由已切换，route.params 会变空
    bookUrl: shelfBook.value.bookUrl || bookUrl.value,
    durChapterIndex: chapterIndex.value,
    durChapterPos: Math.round(currentPos()),
    durChapterTime: Date.now(),
    durChapterTitle: currentChapter.value.title,
    // 同步总章数（书源书入架时可能缺——书架进度百分比依赖它）
    totalChapterNum: realChapters.value.length,
  }).catch(() => {
    /* 静默失败 */
  })
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

/* ---------------- B1 修复：进度恢复双保险（渲染 + 图片 load 后各校正一次） ---------------- */

function settleFrames(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

/** 等待正文内图片加载完成（带超时兜底），避免图片撑高后落点偏移 */
function imagesReady(): Promise<void> {
  const imgs = Array.from(document.querySelectorAll<HTMLImageElement>('.reader-content img'))
  if (imgs.length === 0) return Promise.resolve()
  const wait = (img: HTMLImageElement) =>
    img.complete && img.naturalWidth > 0
      ? Promise.resolve()
      : new Promise<void>((resolve) => {
          const done = () => resolve()
          img.addEventListener('load', done, { once: true })
          img.addEventListener('error', done, { once: true })
          window.setTimeout(done, 3000)
        })
  return Promise.all(imgs.map(wait)).then(() => undefined)
}

async function applyRestoreScroll() {
  if (restoreScrollY == null) return
  const v = flipViewRef.value
  if (isPagedMode() && v) {
    // 仿真翻页：目标为列轴 px；不足时逐块加载直到可分页到目标
    const target = restoreScrollY
    while (hasMoreParagraphs.value) {
      if (v.scrollWidth > target) break
      renderedCount.value = Math.min(renderedCount.value + SEGMENT_SIZE, paragraphs.value.length)
      await nextTick()
      await settleFrames()
    }
    await nextTick()
    await settleFrames()
    const clamp = () => Math.min(target, Math.max(0, v.scrollWidth - v.clientWidth))
    v.scrollTo(0, clamp())
    // 双保险：图片加载（或 3s 超时）后再校正一次
    await imagesReady()
    await settleFrames()
    v.scrollTo(0, clamp())
    restoreScrollY = null
    updateScrollFrac()
    return
  }
  // GAP 155：恢复目标超出当前已渲染高度时，先逐块加载直到可滚动到目标位置
  while (hasMoreParagraphs.value) {
    const doc = document.documentElement
    if (doc.scrollHeight - window.innerHeight >= restoreScrollY) break
    renderedCount.value = Math.min(renderedCount.value + SEGMENT_SIZE, paragraphs.value.length)
    await nextTick()
    await settleFrames()
  }
  await nextTick()
  await settleFrames()
  window.scrollTo(0, restoreScrollY)
  // 双保险：图片加载（或 3s 超时）后再校正一次
  await imagesReady()
  await settleFrames()
  window.scrollTo(0, restoreScrollY)
  restoreScrollY = null
  updateScrollFrac()
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
    // 章节字数：后端 chapterWordCount（本地书正文接口附带）优先；缺失（书源书）用已缓存正文估算
    if (typeof res.data?.chapterWordCount === 'number') {
      chapterWordCounts.value = { ...chapterWordCounts.value, [chapterUrl]: res.data.chapterWordCount }
    } else {
      chapterWordCounts.value = { ...chapterWordCounts.value, [chapterUrl]: content.value.length }
    }
    // 听书：播放中切章 / 本章播完自动连播 → 新章正文就绪后自动续播
    if (ttsState.value !== 'idle' || ttsAutoNext) {
      ttsAutoNext = false
      void startTts()
    }
  } catch {
    ttsAutoNext = false
    loadError.value = true
    return
  } finally {
    loading.value = false
  }
  // 等正文真正渲染（loading 置 false 后）再滚动，避免被加载态高度钳制
  await nextTick()
  if (isPagedMode()) measureFlipColumns()
  if (restoreParagraphIdx != null) {
    await applyRestoreParagraph()
  } else if (isPagedMode()) {
    const v = flipViewRef.value
    if (v) v.scrollTo(0, restoreScrollY ?? 0)
    if (restoreScrollY != null) {
      await applyRestoreScroll()
    } else {
      restoreScrollY = null
      updateScrollFrac()
    }
  } else {
    window.scrollTo(0, restoreScrollY ?? 0)
    if (restoreScrollY != null) {
      await applyRestoreScroll()
    } else {
      restoreScrollY = null
      updateScrollFrac()
    }
  }
  // 当前章渲染后：预加载下一章前 5 张图片（仅当下一章含图片 URL）
  preloadNextChapterImages()
  // 切章后清除朗读高亮（连播/手动切章后由 startTts 重新跟踪）
  ttsReadingPara.value = -1
}

// 临时书（先读后入架）：离开时提醒加入书架
let leavingConfirming = false
onBeforeRouteLeave(async () => {
  const b = shelfBook.value
  if (!b || !(b as unknown as { isTemp?: boolean }).isTemp) return true
  // 正文加载失败（书可能有问题）时离开不打扰——不弹加入书架确认框
  if (loadError.value || notFound.value) return true
  // 防重入：confirm 弹窗挂起期间若再次导航（刷新/连点），旧守卫直接放行，避免多次弹框/守卫悬挂
  if (leavingConfirming) return true
  leavingConfirming = true
  try {
    await ElMessageBox.confirm(`《${b.name || '本书'}》要加入书架吗？加入后可保存阅读进度、续读更方便。`, '加入书架', {
      confirmButtonText: '加入书架',
      cancelButtonText: '暂不加入',
      distinguishCancelAndClose: true,
      type: 'info',
    })
    await saveBook({
      bookUrl: b.bookUrl,
      name: b.name,
      author: b.author,
      origin: b.origin,
      originName: b.originName,
      tocUrl: b.tocUrl,
      intro: b.intro,
      coverUrl: b.coverUrl,
      group: 0,
    } as Book)
    ElMessage.success('已加入书架')
  } catch {
    /* 取消——不加入 */
  }
  return true
})

function goToChapter(idx: number) {
  const ch = chapters.value[idx]
  if (!ch || ch.isVolume) return
  drawerOpen.value = false
  if (idx === chapterIndex.value) return
  // 切章方向（hslide 模式正文滑入过渡动画用）
  chapterDir.value = idx > chapterIndex.value ? 1 : -1
  saveProgress()
  chapterIndex.value = idx
  if (isNonTextBook.value) void loadNonTextChapter(ch.url)
  else void loadContent(ch.url)
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

/* ---------------- GAP 4：长按快进/快退（按住 250ms 后每 300ms 连续翻章；pointer 事件覆盖鼠标/触屏） ---------------- */

interface HoldRepeat {
  start: (e: PointerEvent) => void
  stop: () => void
  /** 长按已触发连翻时吞掉释放后的合成 click（避免多翻一章） */
  swallowClick: () => boolean
}

function createHoldRepeat(fn: () => void): HoldRepeat {
  let timer: number | undefined
  let interval: number | undefined
  let fired = false
  function start(e: PointerEvent) {
    // 仅响应主键（鼠标左键 / 触屏笔尖）
    if (e.pointerType === 'mouse' && e.button !== 0) return
    stop()
    fired = false
    timer = window.setTimeout(() => {
      fired = true
      fn()
      interval = window.setInterval(fn, 300)
    }, 250)
  }
  function stop() {
    if (timer !== undefined) {
      window.clearTimeout(timer)
      timer = undefined
    }
    if (interval !== undefined) {
      window.clearInterval(interval)
      interval = undefined
    }
  }
  function swallowClick(): boolean {
    if (fired) {
      fired = false
      return true
    }
    return false
  }
  return { start, stop, swallowClick }
}

const prevHold = createHoldRepeat(prevChapter)
const nextHold = createHoldRepeat(nextChapter)

function onPrevClick() {
  if (prevHold.swallowClick()) return
  prevChapter()
}

function onNextClick() {
  if (nextHold.swallowClick()) return
  nextChapter()
}

/* ---------------- 点击区域（GAP：整页分三区——左=上一页/上章，右=下一页/下章，中=弹/隐顶底栏） ---------------- */

/** 顶栏 / 底栏 / 顶部进度条显隐（点击中间切换） */
const chromeVisible = ref(true)
function toggleChrome() {
  chromeVisible.value = !chromeVisible.value
}

/** 顶栏/底栏显隐变化 → 分页高度变化，重测分页并保持当前页 */
watch(chromeVisible, () => {
  if (isPagedMode()) {
    void nextTick(() => {
      const v = flipViewRef.value
      const saved = v ? Math.round(v.scrollLeft / flipStep()) : 0
      measureFlipColumns()
      flipGo(saved)
    })
  }
})

/** 上一页/上一章（分页模式 flipPage 翻页；滚动模式切章） */
function pagePrev() {
  if (isPagedMode()) flipPage(-1)
  else prevChapter()
}

/** 下一页/下一章 */
function pageNext() {
  if (isPagedMode()) flipPage(1)
  else nextChapter()
}

/**
 * 整页点击委托：左 1/4 → 上一页/上章；右 1/4 → 下一页/下章；中间 → 弹/隐顶底栏。
 * 仅文本书启用；自动排除：弹层/抽屉/选择条、顶底栏/按钮/图片/输入等自身有交互的元素，
 * 以及拖选/长按选择后的合成 click（selection 非空时忽略）。
 */
function onReaderTap(e: MouseEvent) {
  if (!isTextBook.value) return
  if (e.button !== 0) return
  const t = e.target as HTMLElement | null
  if (!t) return
  // 自身有交互的元素：不吞掉其行为
  if (
    t.closest(
      '.topbar, .reader-toolbar, .progress-bar, .reading-progress, .chapter-nav, ' +
        '.tb-btn, .icon-btn, .font-btn, .nav-btn, .retry-btn, .media-nav, ' +
        '.pop-mask, .pop-card, .drawer-mask, .sel-bar, ' +
        'button, input, textarea, select, a, img',
    )
  ) {
    return
  }
  // 弹层/选择工具条打开时（中间点击会被 mask 自己处理）
  if (selOpen.value) return
  // 拖选/双击选词后的合成 click：忽略，避免误翻页
  const sel = window.getSelection()
  if (sel && sel.toString().trim()) return

  const x = e.clientX
  const w = window.innerWidth
  if (x < w * 0.25) {
    pagePrev()
  } else if (x > w * 0.75) {
    pageNext()
  } else {
    toggleChrome()
  }
}

function retry() {
  if (chapters.value.length === 0) void init()
  else if (currentChapter.value) {
    if (isNonTextBook.value) void loadNonTextChapter(currentChapter.value.url)
    else void loadContent(currentChapter.value.url)
  }
}

/* ---------------- 非文本书渲染：音频/视频播放器 + 漫画逐页 + 文件下载 ---------------- */

/* ---- 音频书 ---- */
const mediaAudioRef = ref<HTMLAudioElement | null>(null)
const audioUrl = ref('')
const audioContentType = ref('')
const audioPlaying = ref(false)
const audioCurrent = ref(0)
const audioDuration = ref(0)
const audioBuffering = ref(false)
/** m3u8 → hls.js 实例（CDN 动态加载；Safari 原生支持时无需） */
let hlsInstance: { destroy: () => void } | null = null
const hlsFailed = ref(false)

/** 动态加载 hls.js（仅 m3u8 且浏览器无原生 HLS 支持时）
 *  P3-A：固定版本 + SRI integrity（防 CDN 投毒；版本浮动 @1 会让 SRI 失效，故钉死 1.5.20） */
function loadHlsJs(): Promise<{ default?: unknown } | null> {
  return new Promise((resolve) => {
    const win = window as unknown as { Hls?: { isSupported: () => boolean; new (): unknown } }
    if (win.Hls) return resolve({ default: win.Hls })
    const script = document.createElement('script')
    script.src = 'https://cdn.jsdelivr.net/npm/hls.js@1.5.20/dist/hls.min.js'
    // sha384 与 jsdelivr 发布的 hls.js@1.5.20 文件一致（本地计算 + registry sha256 双重核验）
    script.integrity =
      'sha384-V5ruNBgmYcC3SJRUQeNykAAAgde5gOFq/Hu0CZj7bygDP0yRIhkvX8+w0u/7mRvr'
    script.crossOrigin = 'anonymous'
    script.onload = () => resolve(win.Hls ? { default: win.Hls } : null)
    script.onerror = () => resolve(null)
    document.head.appendChild(script)
  })
}

async function setupHls(el: HTMLAudioElement, url: string) {
  hlsInstance?.destroy()
  hlsInstance = null
  hlsFailed.value = false
  const win = window as unknown as { Hls?: { isSupported: () => boolean; new (config?: object): { loadSource: (u: string) => void; attachMedia: (m: HTMLMediaElement) => void; destroy: () => void } } }
  // 原生支持（Safari）→ 直接 src
  if (el.canPlayType('application/vnd.apple.mpegurl')) return
  const mod = await loadHlsJs()
  if (!mod || !win.Hls?.isSupported()) {
    hlsFailed.value = true
    return
  }
  const hls = new win.Hls()
  hls.loadSource(url)
  hls.attachMedia(el)
  hlsInstance = hls
}

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) s = 0
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  const h = Math.floor(m / 60)
  if (h > 0) return `${h}:${String(m % 60).padStart(2, '0')}:${String(sec).padStart(2, '0')}`
  return `${m}:${String(sec).padStart(2, '0')}`
}

function onAudioLoadedMeta() {
  const el = mediaAudioRef.value
  if (el && Number.isFinite(el.duration)) audioDuration.value = el.duration
}
function onAudioTimeUpdate() {
  const el = mediaAudioRef.value
  if (!el) return
  audioCurrent.value = el.currentTime
  if (Number.isFinite(el.duration)) audioDuration.value = el.duration
}
function onAudioPlay() {
  audioPlaying.value = true
}
function onAudioPause() {
  audioPlaying.value = false
}
function onAudioWaiting() {
  audioBuffering.value = true
}
function onAudioPlaying() {
  audioBuffering.value = false
}
/** 本章播完 → 自动下一章（与文本书 TTS 连播语义一致） */
function onAudioEnded() {
  audioPlaying.value = false
  if (hasNext.value) nextChapter()
}
function toggleAudioPlay() {
  const el = mediaAudioRef.value
  if (!el || !audioUrl.value) return
  if (el.paused) void el.play().catch(() => { /* 自动播放被拦截——用户已点击，一般可播 */ })
  else el.pause()
}
function seekAudio() {
  const el = mediaAudioRef.value
  if (!el) return
  el.currentTime = audioCurrent.value
}

/* ---- 视频书 ---- */
const videoElRef = ref<HTMLVideoElement | null>(null)
const videoUrl = ref('')
const videoCurrent = ref(0)
const videoDuration = ref(0)
function onVideoTimeUpdate() {
  const el = videoElRef.value
  if (!el) return
  videoCurrent.value = el.currentTime
  if (Number.isFinite(el.duration)) videoDuration.value = el.duration
}

/* ---- 漫画书：横向滑动 + 点击左右边缘翻页 + 懒加载占位 ---- */
const comicScrollRef = ref<HTMLElement | null>(null)
const comicImages = ref<string[]>([])
const comicPage = ref(0)

function onComicScroll() {
  const el = comicScrollRef.value
  if (!el) return
  const page = Math.round(el.scrollLeft / Math.max(1, el.clientWidth))
  comicPage.value = Math.min(comicImages.value.length - 1, Math.max(0, page))
}
/** 点击图片左/右 1/3 区域 → 上一页/下一页（点击中间不翻页，便于看图） */
function comicClickPage(e: MouseEvent, idx: number) {
  const el = comicScrollRef.value
  if (!el) return
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
  const x = e.clientX - rect.left
  const third = rect.width / 3
  if (x < third) {
    if (idx > 0) el.scrollTo({ left: (idx - 1) * el.clientWidth, behavior: 'smooth' })
  } else if (x > third * 2) {
    if (idx < comicImages.value.length - 1)
      el.scrollTo({ left: (idx + 1) * el.clientWidth, behavior: 'smooth' })
  }
}

/* ---- 文件书 ---- */
const fileUrl = ref('')

/** 非文本书章节加载（按 bookType 分派：不走正文分段解析/替换/简繁转换） */
async function loadNonTextChapter(chapterUrl: string) {
  if (!shelfBook.value?.origin) return
  loading.value = true
  loadError.value = false
  // 清空上一章媒体态
  content.value = ''
  audioUrl.value = ''
  videoUrl.value = ''
  fileUrl.value = ''
  comicImages.value = []
  comicPage.value = 0
  hlsFailed.value = false
  try {
    const res = await getBookContent(chapterUrl, shelfBook.value.origin)
    const data = res.data ?? {}
    if (isAudioBook.value) {
      const url = data.audioUrl
      if (!url) {
        loadError.value = true
        return
      }
      audioUrl.value = url
      audioContentType.value = data.contentType ?? ''
      await nextTick()
      const el = mediaAudioRef.value
      if (el) {
        el.pause()
        audioPlaying.value = false
        audioCurrent.value = 0
        audioDuration.value = 0
        el.src = url
        el.load()
        const isHls =
          url.toLowerCase().includes('.m3u8') ||
          (audioContentType.value ?? '').toLowerCase().includes('mpegurl')
        if (isHls) await setupHls(el, url)
        // 恢复该章播放位置（saveProgress 存秒数）
        const savedPos = restoreScrollY ?? 0
        restoreScrollY = null
        if (savedPos > 0) {
          try {
            el.currentTime = savedPos
          } catch {
            /* ignore */
          }
        }
        void el.play().catch(() => {
          audioPlaying.value = false
        })
      }
    } else if (isComicBook.value) {
      const images = data.images
      if (!Array.isArray(images) || images.length === 0) {
        loadError.value = true
        return
      }
      comicImages.value = images
      await nextTick()
      // 恢复该章页位置（saveProgress 存页索引）
      const savedPage = restoreScrollY ?? 0
      restoreScrollY = null
      const el = comicScrollRef.value
      if (el && savedPage > 0 && savedPage < images.length) {
        el.scrollLeft = savedPage * el.clientWidth
        comicPage.value = savedPage
      }
    } else if (isVideoBook.value) {
      videoUrl.value = data.videoUrl ?? ''
      if (!videoUrl.value) {
        loadError.value = true
        return
      }
      await nextTick()
      const el = videoElRef.value
      const savedPos = restoreScrollY ?? 0
      restoreScrollY = null
      if (el && savedPos > 0) {
        el.currentTime = savedPos
      }
    } else if (isFileBook.value) {
      fileUrl.value = data.downloadUrl ?? ''
      if (!fileUrl.value) {
        loadError.value = true
        return
      }
    }
  } catch {
    loadError.value = true
  } finally {
    loading.value = false
  }
}

/** 非文本书播放/暂停（空格键） */
function toggleMediaPlay() {
  if (isAudioBook.value) toggleAudioPlay()
  else if (isVideoBook.value) {
    const el = videoElRef.value
    if (!el) return
    if (el.paused) void el.play().catch(() => {})
    else el.pause()
  }
}

/** 非文本书当前进度位置（音频/视频=秒；漫画=页索引；文件=0） */
function mediaPosition(): number {
  if (isAudioBook.value) return mediaAudioRef.value?.currentTime ?? audioCurrent.value
  if (isVideoBook.value) return videoElRef.value?.currentTime ?? videoCurrent.value
  if (isComicBook.value) return comicPage.value
  return 0
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
    if (!found?.origin) {
      // 非书架书（详情页「直接阅读」跳入）：用 query 构建临时书——先读后提示入架
      const q = route.query as Record<string, string>
      if (q.source) {
        const qType = Number(q.type)
        shelfBook.value = {
          bookUrl: bookUrl.value,
          origin: q.source,
          originName: q.sourceName || '',
          tocUrl: q.toc || bookUrl.value,
          name: q.name || '未命名',
          // 非文本书临时直读：详情页透传 type（0 文本/1 音频/2 漫画/3 文件/4 视频）
          type: Number.isInteger(qType) && qType >= 0 && qType <= 4 ? qType : 0,
          group: 0,
        } as Book
        shelfBook.value.isTemp = true as never
        bookName.value = shelfBook.value.name
      } else {
        notFound.value = true
        return
      }
    }
    // 本地书 tocUrl 可能为空——用 bookUrl 兜底
    if (found && !found.tocUrl) found.tocUrl = found.bookUrl
    if (found) {
      shelfBook.value = found
      bookName.value = found.name
    }
    // 替换规则：进入阅读页时读取一次（localStorage 占位；后端就绪后走 GET /reader3/getReplaceRules）
    if (replaceEnabled.value) refreshReplaceRules()

    // 先取详情拿到真实 tocUrl（ruleBookInfo 计算的 /chapters?paging=0 等），
    // 再取目录——临时书 tocUrl 默认是 bookUrl（详情页），直接请求目录会抓到详情导致"第 1/1 章"+正文 JSON
    const infoRes = await getBookInfo(shelfBook.value!.bookUrl, shelfBook.value!.origin, {
      silent: true,
    }).catch(() => null)
    const infoData = infoRes?.isSuccess ? (infoRes.data as { tocUrl?: string; name?: string } | null) : null
    if (infoData?.tocUrl) {
      shelfBook.value!.tocUrl = infoData.tocUrl
    }
    const tocRes = await getBookToc(shelfBook.value!.tocUrl, shelfBook.value!.origin)
    if (tocRes.isSuccess) {
      chapters.value = tocRes.data ?? []
    } else {
      loadError.value = true
      return
    }
    if (infoData?.name) {
      bookName.value = infoData.name
    }

    // 起始章节：① 全书搜索跳转 ?chapter=index 显式指定（优先级最高）；
    // ② 服务端进度（getBookshelf 已带 durChapter*；durChapterTime>0 才算存过）；
    // ③ 无服务端进度则回退 localStorage
    let startIndex = realChapters.value.length ? chapters.value.indexOf(realChapters.value[0]) : 0
    const qc = Number(route.query.chapter)
    const queryChapter =
      Number.isInteger(qc) && qc >= 0 && qc < chapters.value.length && !chapters.value[qc].isVolume
        ? qc
        : -1
    const srvIdx = shelfBook.value?.durChapterIndex ?? -1
    const serverSaved =
      typeof srvIdx === 'number' &&
      srvIdx >= 0 &&
      srvIdx < chapters.value.length &&
      !chapters.value[srvIdx].isVolume &&
      (shelfBook.value?.durChapterTime ?? 0 ?? 0) > 0
    if (queryChapter >= 0) {
      startIndex = queryChapter
    } else if (serverSaved) {
      startIndex = srvIdx
      const srvPos = shelfBook.value?.durChapterPos ?? 0 ?? 0
      restoreScrollY = srvPos > 0 ? srvPos : 0
    } else {
      const saved = restoreProgress()
      if (
        saved &&
        saved.chapterIndex >= 0 &&
        saved.chapterIndex < chapters.value.length &&
        !chapters.value[saved.chapterIndex].isVolume
      ) {
        startIndex = saved.chapterIndex
        restoreScrollY = saved.scrollY
      }
    }
    chapterIndex.value = startIndex
    const start = chapters.value[startIndex]
    if (start) {
      // 非文本书：按类型分派（音频/视频/漫画/文件）——不走正文分段解析
      if (isNonTextBook.value) await loadNonTextChapter(start.url)
      else await loadContent(start.url)
    }
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

/* ---------------- B2 修复：目录抽屉打开时定位当前章（.current 高亮 + scrollIntoView block:center，仅打开时） ---------------- */

const drawerListRef = ref<HTMLElement | null>(null)
watch(drawerOpen, async (open) => {
  if (!open) return
  await nextTick()
  await nextTick()
  const list = drawerListRef.value
  const el = list?.querySelector<HTMLElement>('.chapter-item.current')
  if (list && el) {
    // 抽屉为 fixed 定位容器，scrollIntoView 只会滚动抽屉内部列表，不会带动页面
    el.scrollIntoView({ block: 'center' })
  }
})

/* ---------------- 键盘翻页（←/→ 翻章；PageUp/PageDown 滚动；Space 自动阅读暂停/恢复；输入框聚焦时不触发） ---------------- */

function isTypingTarget(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false
  if (el.isContentEditable) return true
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT'
}

function onKeydown(e: KeyboardEvent) {
  if (isTypingTarget(e.target)) return
  if (e.metaKey || e.ctrlKey || e.altKey) return
  switch (e.code) {
    case 'ArrowLeft':
      // 目录抽屉打开时 ←/→ 不翻章（避免浏览目录时误翻）
      if (drawerOpen.value) return
      e.preventDefault()
      if (isPagedMode()) flipPage(-1)
      else prevChapter()
      break
    case 'ArrowRight':
      if (drawerOpen.value) return
      e.preventDefault()
      if (isPagedMode()) flipPage(1)
      else nextChapter()
      break
    case 'PageUp':
      e.preventDefault()
      if (isPagedMode()) flipPage(-1)
      else window.scrollBy({ top: -window.innerHeight * 0.9, behavior: 'auto' })
      break
    case 'PageDown':
      e.preventDefault()
      if (isPagedMode()) flipPage(1)
      else window.scrollBy({ top: window.innerHeight * 0.9, behavior: 'auto' })
      break
    case 'Space':
      // 非文本书：空格 = 播放/暂停；文本书：仿真翻页翻页 / 其余模式自动阅读暂停/恢复（阻止默认的页面滚动）
      e.preventDefault()
      if (isNonTextBook.value) toggleMediaPlay()
      else if (isPagedMode()) flipPage(1)
      else toggleAuto()
      break
    case 'Escape':
      // 图片全屏查看：Esc 关闭
      if (imgViewerOpen.value) {
        e.preventDefault()
        closeImgViewer()
      }
      break
  }
}

/* ---------------- 生命周期 ---------------- */

function onScroll() {
  updateScrollFrac()
  hideSelBar()
  // GAP 155：滚动近底时渐进加载下一段
  maybeLoadMoreChunk()
  window.clearTimeout(saveTimer)
  saveTimer = window.setTimeout(saveProgress, 300)
}

onMounted(() => {
  // 跟随系统：matchMedia 监听系统深色偏好（先于 applyTheme，保证 system 主题首帧正确）
  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  systemDark.value = mediaQuery.matches
  mediaQuery.addEventListener('change', onSystemThemeChange)
  applyTheme(theme.value)
  window.addEventListener('scroll', onScroll, { passive: true })
  window.addEventListener('resize', onResize, { passive: true })
  window.addEventListener('mouseup', onSelectionUp)
  window.addEventListener('touchend', onSelectionUp, { passive: true })
  window.addEventListener('mousedown', onDocMouseDown)
  window.addEventListener('keydown', onKeydown)
  // GAP 85：左右滑动翻章（passive——不 preventDefault，不干扰纵向滚动/选择）
  window.addEventListener('touchstart', onSwipeTouchStart, { passive: true })
  window.addEventListener('touchend', onSwipeTouchEnd, { passive: true })
  if (pageMode.value === 'slide' || pageMode.value === 'flip') {
    window.addEventListener('wheel', onWheel, { passive: false })
    window.addEventListener('touchstart', onTouchStart, { passive: true })
    window.addEventListener('touchmove', onTouchMove, { passive: false })
    window.addEventListener('touchend', onTouchEnd, { passive: true })
  }
  // 屏幕常亮（Wake Lock）：进入阅读页且页面可见时请求；切后台自动释放，回前台自动恢复
  document.addEventListener('visibilitychange', onWakeVisibilityChange)
  if (wakeLockEnabled.value && document.visibilityState === 'visible') void requestWakeLock()
  // GAP 110：每日阅读时长累计（本机）
  startDailyTracker()
  void init()
})

onBeforeUnmount(() => {
  mediaQuery?.removeEventListener('change', onSystemThemeChange)
  window.removeEventListener('scroll', onScroll)
  window.removeEventListener('resize', onResize)
  window.removeEventListener('wheel', onWheel)
  window.removeEventListener('touchstart', onTouchStart)
  window.removeEventListener('touchmove', onTouchMove)
  window.removeEventListener('touchend', onTouchEnd)
  window.removeEventListener('mouseup', onSelectionUp)
  window.removeEventListener('touchend', onSelectionUp)
  window.removeEventListener('mousedown', onDocMouseDown)
  window.removeEventListener('keydown', onKeydown)
  // GAP 85：左右滑动翻章监听清理
  window.removeEventListener('touchstart', onSwipeTouchStart)
  window.removeEventListener('touchend', onSwipeTouchEnd)
  window.clearTimeout(saveTimer)
  window.clearTimeout(flashTimer)
  prevHold.stop()
  nextHold.stop()
  stopTts()
  stopAuto()
  stopDailyTracker()
  // 屏幕常亮：离开阅读页释放
  document.removeEventListener('visibilitychange', onWakeVisibilityChange)
  void releaseWakeLock()
  // 非文本书：暂停媒体 + 销毁 hls.js 实例
  mediaAudioRef.value?.pause()
  videoElRef.value?.pause()
  hlsInstance?.destroy()
  hlsInstance = null
  saveProgress()
})
</script>

<template>
  <div
    ref="pageRef"
    class="reader-page"
    :class="{ texture: effectiveTexture, 'flip-layout': isPagedMode() }"
    :style="pageStyle"
    @click="onReaderTap"
  >
    <!-- GAP 149：顶部细进度条（scroll 比例，1px 强调色；点击可跳章） -->
    <button
      v-if="!loading && !loadError && !notFound && realChapters.length > 0"
      v-show="chromeVisible"
      class="reading-progress"
      type="button"
      :title="'跳转章节'"
      @click="jumpOpen = true"
    >
      <i class="reading-progress-fill" :style="{ width: `${progressPct}%` }"></i>
    </button>

    <!-- 顶部极简栏 -->
    <header class="topbar" v-show="chromeVisible">
      <button class="icon-btn" type="button" :title="'返回'" @click="goBack">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5" />
          <path d="M11 18l-6-6 6-6" />
        </svg>
      </button>

      <span class="book-name" :title="displayBookName">{{ displayBookName || '阅读' }}</span>

      <div class="top-actions">
        <button
          class="font-btn"
          type="button"
          :class="{ active: brightness !== 1 }"
          :title="'亮度（0.6-1.4 倍）'"
          @click="brightnessOpen = true"
        >
          {{ '亮度' }}
        </button>
        <button
          v-if="isTextBook"
          class="font-btn tts-btn"
          type="button"
          :class="{ active: ttsPlaying }"
          :title="
            ttsState === 'playing' || ttsState === 'loading'
              ? '停止朗读'
              : ttsState === 'paused'
                ? '继续朗读'
                : '听书（后端语音合成，本章播完自动下一章）'
          "
          @click="toggleTts"
        >
          {{ ttsTopLabel }}
        </button>
        <button class="font-btn more-btn" type="button" :title="'更多'" @click="moreOpen = !moreOpen">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
            <circle cx="12" cy="5" r="1.2" />
            <circle cx="12" cy="12" r="1.2" />
            <circle cx="12" cy="19" r="1.2" />
          </svg>
        </button>
      </div>
    </header>

    <!-- 正文 -->
    <main
      class="reader-main"
      :style="{ maxWidth: contentWidth, filter: `brightness(${brightness})` }"
    >
      <!-- 不在书架 -->
      <div v-if="notFound" class="state">
        <p class="state-text">{{ '未找到这本书（可能不在书架中）' }}</p>
        <button class="retry-btn" type="button" @click="router.replace('/')">{{ '返回书架' }}</button>
      </div>

      <!-- 目录为空 -->
      <div v-else-if="!loading && !loadError && chapters.length === 0" class="state">
        <p class="state-text">{{ '未获取到章节目录' }}</p>
        <button class="retry-btn" type="button" @click="retry">{{ '重试' }}</button>
      </div>

      <template v-else>
        <h1 v-if="currentChapter" class="chapter-title">{{ displayChapterTitle }}</h1>

        <!-- ============ 文本书：现有正文链路 ============ -->
        <template v-if="isTextBook">
          <!-- 加载态：细字 -->
          <div v-if="loading" class="state">
            <p class="state-text loading-text">{{ '加载中…' }}</p>
          </div>

          <!-- 错误态 -->
          <div v-else-if="loadError" class="state">
            <p class="state-text">{{ '正文获取失败，请稍后重试' }}</p>
            <button class="retry-btn" type="button" @click="retry">{{ '重试' }}</button>
          </div>

          <!-- 空内容 -->
          <div v-else-if="paragraphs.length === 0" class="state">
            <p class="state-text">{{ '本章暂无内容' }}</p>
          </div>

          <!-- 正文（分页模式：横向分页容器；其余模式：普通纵向流） -->
          <div
            v-else
            ref="flipViewRef"
            class="flip-view"
            :class="{ active: isPagedMode() }"
            @scroll.passive="onFlipScroll"
          >
            <article class="reader-content" :class="chapterAnimClass" :style="contentStyle">
            <template v-for="(para, i) in visibleParagraphs" :key="i">
              <!-- 单张图片段落（GAP 102）：渲染图片，点击全屏查看 -->
              <img
                v-if="visibleParaImgs[i]"
                v-lazy="visibleParaImgs[i] as string"
                class="reader-img"
                :alt="`正文图片 ${i + 1}`"
                loading="lazy"
                @click="openImgViewer(visibleParaImgs[i] as string)"
              />
              <p
                v-else
                class="reader-para"
                :data-para="i"
                :class="{ flash: flashParaIdx === i, 'tts-reading': ttsReadingPara === i }"
                :style="{ marginBottom: `${paraSpacing}em`, textIndent: textIndent ? '2em' : '0' }"
              >
                <span v-if="paraDisplayHtml(i) !== null" v-html="paraDisplayHtml(i)"></span>
                <template v-else>{{ para }}</template>
              </p>
            </template>
            </article>
          </div>

          <!-- 底部极简导航（长按快进/快退：按住 250ms 后每 300ms 连续翻章） -->
          <nav class="chapter-nav">
            <button
              class="nav-btn"
              type="button"
              :disabled="!hasPrev"
              :title="'上一章（长按连续快退）'"
              @click="onPrevClick"
              @pointerdown="prevHold.start"
              @pointerup="prevHold.stop"
              @pointerleave="prevHold.stop"
              @pointercancel="prevHold.stop"
            >
              {{ '上一章' }}
            </button>
            <button
              class="nav-btn"
              type="button"
              :disabled="!hasNext"
              :title="'下一章（长按连续快进）'"
              @click="onNextClick"
              @pointerdown="nextHold.start"
              @pointerup="nextHold.stop"
              @pointerleave="nextHold.stop"
              @pointercancel="nextHold.stop"
            >
              {{ '下一章' }}
            </button>
          </nav>
        </template>

        <!-- ============ 非文本书（音频/漫画/视频/文件——按 book_type 分派） ============ -->
        <template v-else>
          <!-- 加载态 -->
          <div v-if="loading" class="state">
            <p class="state-text loading-text">{{ '加载中…' }}</p>
          </div>

          <!-- 错误态 -->
          <div v-else-if="loadError" class="state">
            <p class="state-text">内容获取失败，请稍后重试</p>
            <button class="retry-btn" type="button" @click="retry">{{ '重试' }}</button>
          </div>

          <!-- 音频书：播放器（播放/暂停/进度/上一章下一章；m3u8 走 hls.js） -->
          <div v-else-if="isAudioBook" class="media-stage audio-stage">
            <div class="media-card">
              <div
                class="media-art"
                :style="shelfBook?.coverUrl || shelfBook?.customCoverUrl ? { backgroundImage: `url(${shelfBook?.customCoverUrl || shelfBook?.coverUrl})` } : {}"
              >
                <span class="media-badge">{{ '音频书' }}</span>
                <span v-if="audioBuffering" class="media-buffering">{{ '缓冲中…' }}</span>
              </div>
              <p class="media-chapter">{{ displayChapterTitle }}</p>
              <div class="media-controls">
                <button class="media-nav" type="button" :disabled="!hasPrev" :title="'上一章'" @click="prevChapter">
                  {{ '上一章' }}
                </button>
                <button
                  class="media-play"
                  type="button"
                  :disabled="!audioUrl"
                  :title="audioPlaying ? '暂停' : '播放'"
                  @click="toggleAudioPlay"
                >
                  {{ audioPlaying ? '❚❚' : '▶' }}
                </button>
                <button class="media-nav" type="button" :disabled="!hasNext" :title="'下一章'" @click="nextChapter">
                  {{ '下一章' }}
                </button>
              </div>
              <div class="media-progress">
                <span class="media-time">{{ fmtTime(audioCurrent) }}</span>
                <input
                  class="media-slider"
                  type="range"
                  min="0"
                  :max="audioDuration || 0"
                  step="1"
                  v-model.number="audioCurrent"
                  :disabled="!audioDuration"
                  @input="seekAudio"
                />
                <span class="media-time">{{ fmtTime(audioDuration) }}</span>
              </div>
              <p v-if="hlsFailed" class="media-hint">
                {{ 'm3u8 需要 HLS 支持：浏览器不支持且 hls.js 加载失败（请检查网络）' }}
              </p>
              <p class="media-hint">{{ '本章播完自动连播下一章 · 空格键播放/暂停' }}</p>
            </div>
          </div>

          <!-- 漫画书：横向滑动 + 点击左右边缘翻页 + 懒加载占位 -->
          <div v-else-if="isComicBook" class="media-stage comic-stage">
            <div v-if="comicImages.length === 0" class="state">
              <p class="state-text">{{ '本章暂无图片' }}</p>
            </div>
            <template v-else>
              <div ref="comicScrollRef" class="comic-scroll" @scroll="onComicScroll">
                <div
                  v-for="(img, i) in comicImages"
                  :key="`${currentChapter?.url}-${i}`"
                  class="comic-page"
                  :class="{ active: i === comicPage }"
                >
                  <!-- 加载中占位（懒加载完成前显示） -->
                  <div class="comic-placeholder">{{ '加载中…' }}</div>
                  <img
                    v-lazy="img"
                    class="comic-img"
                    :alt="`第 ${i + 1} 页`"
                    loading="lazy"
                    @click="comicClickPage($event, i)"
                  />
                </div>
              </div>
              <div class="comic-foot">
                <span class="comic-hint">{{ '← 点击左右边缘翻页 / 横向滑动浏览 →' }}</span>
                <span class="comic-page-indicator">{{ `第 ${comicPage + 1} / ${comicImages.length} 页` }}</span>
              </div>
            </template>
          </div>

          <!-- 视频书：原生视频播放器 -->
          <div v-else-if="isVideoBook" class="media-stage video-stage">
            <div v-if="!videoUrl" class="state">
              <p class="state-text">{{ '未获取到视频地址' }}</p>
            </div>
            <video
              v-else
              ref="videoElRef"
              class="video-player"
              :src="videoUrl"
              controls
              autoplay
              playsinline
              preload="metadata"
              @timeupdate="onVideoTimeUpdate"
            ></video>
          </div>

          <!-- 文件书：下载按钮 + 说明 -->
          <div v-else-if="isFileBook" class="media-stage file-stage">
            <div class="file-card">
              <p class="file-title">{{ displayBookName }}</p>
              <p class="file-intro">{{ shelfBook?.customIntro || shelfBook?.intro || '文件型书籍：点击下方按钮下载文件阅读。' }}</p>
              <a
                v-if="fileUrl"
                class="download-btn"
                :href="fileUrl"
                target="_blank"
                rel="noopener noreferrer"
                :download="displayBookName || 'download'"
              >
                {{ '下载文件' }}
              </a>
              <p v-else class="media-hint">{{ '未获取到下载链接' }}</p>
            </div>
          </div>

          <!-- 底部导航（长按快进/快退） -->
          <nav v-if="realChapters.length > 0" class="chapter-nav">
            <button
              class="nav-btn"
              type="button"
              :disabled="!hasPrev"
              :title="'上一章（长按连续快退）'"
              @click="onPrevClick"
              @pointerdown="prevHold.start"
              @pointerup="prevHold.stop"
              @pointerleave="prevHold.stop"
              @pointercancel="prevHold.stop"
            >
              {{ '上一章' }}
            </button>
            <button
              class="nav-btn"
              type="button"
              :disabled="!hasNext"
              :title="'下一章（长按连续快进）'"
              @click="onNextClick"
              @pointerdown="nextHold.start"
              @pointerup="nextHold.stop"
              @pointerleave="nextHold.stop"
              @pointercancel="nextHold.stop"
            >
              下一章
            </button>
          </nav>
        </template>
      </template>
    </main>

    <!-- 底部工具栏（图标+文字上下布局：字号 / 主题 / 目录 / 设置） -->
    <div
      v-if="!loading && !loadError && !notFound && realChapters.length > 0"
      v-show="chromeVisible"
      class="reader-toolbar"
    >
      <button
        v-if="isTextBook"
        class="tb-btn"
        type="button"
        :disabled="fontSize <= MIN_FONT"
        :title="'减小字号'"
        @click="fontSize = Math.max(MIN_FONT, fontSize - 1)"
      >
        <span class="tb-icon">A−</span>
        <span class="tb-label">{{ '减小字号' }}</span>
      </button>
      <button
        v-if="isTextBook"
        class="tb-btn"
        type="button"
        :disabled="fontSize >= MAX_FONT"
        :title="'增大字号'"
        @click="fontSize = Math.min(MAX_FONT, fontSize + 1)"
      >
        <span class="tb-icon">A+</span>
        <span class="tb-label">{{ '增大字号' }}</span>
      </button>
      <button class="tb-btn" type="button" :title="`主题：${themeName(theme)}（点击切换）`" @click="themeOpen = true">
        <span class="tb-icon tb-theme-dot" :class="'dot-' + theme"></span>
        <span class="tb-label">{{ themeName(theme) }}</span>
      </button>
      <button class="tb-btn" type="button" :title="'目录'" @click="drawerOpen = true">
        <svg class="tb-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M4 6h16" />
          <path d="M4 12h16" />
          <path d="M4 18h10" />
        </svg>
        <span class="tb-label">{{ '目录' }}</span>
      </button>
      <button class="tb-btn" type="button" :title="'阅读设置'" @click="openSettings">
        <svg class="tb-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M4 6h16" />
          <path d="M4 12h16" />
          <path d="M4 18h16" />
          <circle cx="9" cy="6" r="2" />
          <circle cx="15" cy="12" r="2" />
          <circle cx="7" cy="18" r="2" />
        </svg>
        <span class="tb-label">{{ '设置' }}</span>
      </button>
    </div>

    <!-- 进度：底部细字 + 可点击跳章 -->
    <button
      v-if="!loading && !loadError && !notFound && realChapters.length > 0"
      v-show="chromeVisible"
      class="progress-bar"
      type="button"
      title="跳转章节"
      @click="jumpOpen = true"
    >
      <span class="progress-track"><i class="progress-fill" :style="{ width: `${progressPct}%` }"></i></span>
      <span class="progress-text">第 {{ flatIndex + 1 }}/{{ realChapters.length }} 章 · {{ progressPct }}%</span>
    </button>

    <!-- 章节跳转弹层 -->
    <transition name="pop">
      <div v-if="jumpOpen" class="pop-mask" @click="jumpOpen = false">
        <div class="pop-card" @click.stop>
          <p class="pop-title">跳转章节</p>
          <p class="pop-hint">共 {{ realChapters.length }} 章（1 – {{ realChapters.length }}）</p>
          <div class="pop-row">
            <input
              v-model="jumpNum"
              class="pop-input"
              type="number"
              min="1"
              :max="realChapters.length"
              placeholder="章节号"
              @keyup.enter="confirmJump"
            />
            <button class="pop-btn" type="button" @click="confirmJump">跳转</button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 页内搜索弹层（本地搜索当前章：正文 <mark> 高亮 + 上一个/下一个跳转 + 计数） -->
    <transition name="pop">
      <div v-if="searchOpen" class="pop-mask" @click="closeChapterSearch">
        <div class="pop-card search-card" @click.stop>
          <p class="pop-title">搜索本章</p>
          <div class="pop-row">
            <input
              ref="searchInputRef"
              v-model="searchKeyword"
              class="pop-input"
              type="text"
              placeholder="输入关键词，搜索当前章正文"
              spellcheck="false"
              @keyup.enter="runChapterSearch"
              @keyup.esc="closeChapterSearch"
            />
            <button class="pop-btn" type="button" @click="runChapterSearch">搜索</button>
          </div>
          <p v-if="searchSearched && !searchKeyword.trim()" class="pop-hint">请输入关键词</p>
          <p v-else-if="searchSearched && matchTotal === 0" class="pop-hint">
            本章未找到「{{ searchKeyword.trim() }}」
          </p>
          <template v-else-if="searchSearched && matchTotal > 0">
            <p class="pop-hint">
              共 {{ matchTotal }} 处命中 · 当前第 {{ curMatch + 1 }} 处（正文已高亮，上一个/下一个跳转）
            </p>
            <div class="search-nav">
              <button class="pop-btn" type="button" @click="prevMatch">上一个</button>
              <button class="pop-btn" type="button" @click="nextMatch">下一个</button>
              <button class="text-btn" type="button" @click="closeChapterSearch">完成</button>
            </div>
          </template>
          <p v-else class="pop-hint">搜索范围：当前章（前端本地匹配，正文高亮，上一个/下一个跳转）</p>
        </div>
      </div>
    </transition>

    <!-- 亮度弹层（滑条 0.6-1.4，filter: brightness() 作用于 .reader-main） -->
    <transition name="pop">
      <div v-if="brightnessOpen" class="pop-mask" @click="brightnessOpen = false">
        <div class="pop-card" @click.stop>
          <p class="pop-title">亮度</p>
          <p class="pop-hint">0.6 – 1.4 倍 · 作用于正文区域</p>
          <div class="bright-row">
            <input
              v-model.number="brightness"
              class="bright-slider"
              type="range"
              min="0.6"
              max="1.4"
              step="0.05"
              @input="brightness = round2(Number(brightness))"
            />
            <span class="bright-value">{{ brightness.toFixed(2) }}</span>
          </div>
          <div class="set-foot">
            <button class="text-btn" type="button" @click="brightness = 1">恢复默认</button>
            <button class="pop-btn" type="button" @click="brightnessOpen = false">关闭</button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 阅读设置弹层（GAP 6：全局 / 本书 两个 tab——本书设置 12 项 per-book 覆盖，优先于全局） -->
    <transition name="pop">
      <div v-if="settingsOpen" class="pop-mask" @click="confirmSettings">
        <div class="pop-card settings-card" @click.stop>
          <p class="pop-title">阅读设置</p>

          <div class="cfg-tabs">
            <button
              class="cfg-tab"
              :class="{ active: bookCfgTab === 'global' }"
              type="button"
              title="修改对所有书生效"
              @click="bookCfgTab = 'global'"
            >
              全局
            </button>
            <button
              class="cfg-tab"
              :class="{ active: bookCfgTab === 'book' }"
              type="button"
              title="仅本书记忆，优先于全局"
              @click="bookCfgTab = 'book'"
            >
              本书设置
            </button>
          </div>
          <p v-if="bookCfgTab === 'book'" class="pop-hint">本书设置优先于全局（12 项仅本书记忆）</p>

          <!-- 本书专属：主题 / 字号 / 简繁 -->
          <div v-if="bookCfgTab === 'book'" class="set-row">
            <span class="set-label">主题</span>
            <div class="seg">
              <button
                v-for="th in THEME_ORDER"
                :key="th"
                class="seg-btn"
                :class="{ active: theme === th }"
                type="button"
                @click="selectTheme(th)"
              >
                {{ themeName(th) }}
              </button>
            </div>
          </div>

          <div v-if="bookCfgTab === 'book'" class="set-row">
            <span class="set-label">字号</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="fontSize <= MIN_FONT"
                @click="fontSize = Math.max(MIN_FONT, fontSize - 1)"
              >
                −
              </button>
              <span class="set-value">{{ fontSize }}px</span>
              <button
                class="set-btn"
                type="button"
                :disabled="fontSize >= MAX_FONT"
                @click="fontSize = Math.min(MAX_FONT, fontSize + 1)"
              >
                ＋
              </button>
            </div>
          </div>

          <div v-if="bookCfgTab === 'book'" class="set-row">
            <span class="set-label">简繁</span>
            <div class="seg">
              <button
                class="seg-btn"
                :class="{ active: hanMode === 'auto' }"
                type="button"
                @click="hanMode = 'auto'"
              >
                自动
              </button>
              <button
                class="seg-btn"
                :class="{ active: hanMode === 'simp' }"
                type="button"
                @click="hanMode = 'simp'"
              >
                简体
              </button>
              <button
                class="seg-btn"
                :class="{ active: hanMode === 'trad' }"
                type="button"
                @click="hanMode = 'trad'"
              >
                繁体
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">行距</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="lineHeight <= MIN_LINE"
                @click="lineHeight = round1(lineHeight - 0.1)"
              >
                −
              </button>
              <span class="set-value">{{ lineHeight.toFixed(1) }}</span>
              <button
                class="set-btn"
                type="button"
                :disabled="lineHeight >= MAX_LINE"
                @click="lineHeight = round1(lineHeight + 0.1)"
              >
                ＋
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">段距</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="paraSpacing <= MIN_PARA"
                @click="paraSpacing = round1(paraSpacing - 0.1)"
              >
                −
              </button>
              <span class="set-value">{{ paraSpacing.toFixed(1) }}</span>
              <button
                class="set-btn"
                type="button"
                :disabled="paraSpacing >= MAX_PARA"
                @click="paraSpacing = round1(paraSpacing + 0.1)"
              >
                ＋
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">字重</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="fontWeight <= MIN_WEIGHT"
                @click="fontWeight = Math.max(MIN_WEIGHT, fontWeight - 50)"
              >
                −
              </button>
              <span class="set-value">{{ fontWeight }}</span>
              <button
                class="set-btn"
                type="button"
                :disabled="fontWeight >= MAX_WEIGHT"
                @click="fontWeight = Math.min(MAX_WEIGHT, fontWeight + 50)"
              >
                ＋
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">字体</span>
            <div class="font-picker">
              <button
                type="button"
                class="font-trigger"
                :style="{ fontFamily: fontFamilyStyle || undefined }"
                @click="fontOpen = !fontOpen"
              >
                {{ fontLabel }}
                <span class="font-caret" :class="{ open: fontOpen }">▾</span>
              </button>
              <transition name="fade-drop">
                <div v-if="fontOpen" class="font-menu">
                  <button
                    v-for="opt in FONT_OPTIONS"
                    :key="opt.value"
                    type="button"
                    class="font-item"
                    :class="{ active: fontKind === opt.value }"
                    :style="{ fontFamily: FONT_STACK[opt.value] || undefined }"
                    @click="fontKind = opt.value; fontOpen = false"
                  >
                    <span class="font-item-name">{{ opt.label }}</span>
                    <span class="font-item-demo">永州之野产异蛇，黑质而白章</span>
                  </button>
                </div>
              </transition>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">字距</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="letterSpacing <= 0"
                @click="letterSpacing = round1(Math.max(0, letterSpacing - 0.5))"
              >
                −
              </button>
              <span class="set-value">{{ letterSpacing }}</span>
              <button
                class="set-btn"
                type="button"
                :disabled="letterSpacing >= 2"
                @click="letterSpacing = round1(Math.min(2, letterSpacing + 0.5))"
              >
                ＋
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">缩进</span>
            <div class="seg">
              <button
                class="seg-btn"
                type="button"
                :class="{ active: !textIndent }"
                @click="textIndent = false"
              >
                无
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: textIndent }"
                @click="textIndent = true"
              >
                2em
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">对齐</span>
            <div class="seg">
              <button
                class="seg-btn"
                type="button"
                :class="{ active: textAlign === 'left' }"
                @click="textAlign = 'left'"
              >
                左
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: textAlign === 'justify' }"
                @click="textAlign = 'justify'"
              >
                两端
              </button>
            </div>
          </div>

          <div v-if="bookCfgTab === 'global'" class="set-row">
            <span class="set-label">纸纹</span>
            <div class="set-controls">
              <button
                class="switch"
                :class="{ on: effectiveTexture }"
                type="button"
                role="switch"
                :aria-checked="effectiveTexture"
                :title="effectiveTexture ? '关闭纸纹' : '开启纸纹（细噪点微纹理，暖色主题效果最佳）'"
                @click="toggleTexture"
              >
                <span class="switch-knob"></span>
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">翻页</span>
            <div class="seg compact">
              <button
                class="seg-btn"
                type="button"
                :class="{ active: pageMode === 'scroll' }"
                title="连续滚动（默认）"
                @click="pageMode = 'scroll'"
              >
                滚动
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: pageMode === 'hslide' }"
                title="左右滑动翻章（带切章过渡动画）"
                @click="pageMode = 'hslide'"
              >
                左右
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: pageMode === 'slide' }"
                title="上下翻页（滚轮/触屏/按钮逐屏滚动）"
                @click="pageMode = 'slide'"
              >
                上下
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: pageMode === 'flip' }"
                title="仿真翻页（横向页过渡）"
                @click="pageMode = 'flip'"
              >
                仿真
              </button>
            </div>
          </div>

          <div v-if="bookCfgTab === 'global'" class="set-row">
            <span class="set-label">屏幕常亮</span>
            <div class="set-controls">
              <button
                class="switch"
                :class="{ on: wakeLockEnabled }"
                type="button"
                role="switch"
                :aria-checked="wakeLockEnabled"
                :title="wakeLockEnabled ? '关闭屏幕常亮（阅读时允许锁屏）' : '开启屏幕常亮（阅读时保持亮屏）'"
                @click="wakeLockEnabled = !wakeLockEnabled"
              >
                <span class="switch-knob"></span>
              </button>
            </div>
          </div>

          <div v-if="bookCfgTab === 'global'" class="set-row">
            <span class="set-label">替换规则</span>
            <div class="set-controls">
              <button
                class="switch"
                :class="{ on: replaceEnabled }"
                type="button"
                role="switch"
                :aria-checked="replaceEnabled"
                :title="replaceEnabled ? '关闭正文替换' : '开启正文替换'"
                @click="replaceEnabled = !replaceEnabled"
              >
                <span class="switch-knob"></span>
              </button>
              <button class="manage-link" type="button" title="管理替换规则" @click="router.push('/rules')">
                管理
              </button>
            </div>
          </div>

          <div v-if="bookCfgTab === 'global'" class="set-row">
            <span class="set-label">自动速度</span>
            <div class="seg-row">
              <button
                v-for="s in 5"
                :key="s"
                type="button"
                class="seg-btn"
                :class="{ active: autoSpeed === s }"
                :title="`${s}：约每 ${6 - s} 秒滚一行`"
                @click="autoSpeed = s"
              >
                {{ s }}
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">宽度</span>
            <div class="seg-row">
              <button
                v-for="opt in WIDTH_OPTIONS"
                :key="opt.value"
                type="button"
                class="seg-btn"
                :class="{ active: contentWidth === opt.value }"
                @click="setWidth(opt.value)"
              >
                {{ opt.label }}
              </button>
            </div>
          </div>
          <div class="set-foot">
            <button v-if="bookCfgTab === 'global'" class="text-btn" type="button" @click="resetTypography">
              恢复默认
            </button>
            <button v-else class="text-btn" type="button" title="清除本书设置，恢复使用全局阅读偏好" @click="restoreGlobalDefaults">
              恢复全局默认
            </button>
            <span class="set-foot-spacer"></span>
            <button class="text-btn" type="button" @click="cancelSettings">取消</button>
            <button class="pop-btn" type="button" @click="confirmSettings">确认</button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 主题预设弹层（底部工具栏「主题」：白 / 米黄 / 绿 / 灰黑夜 / 纯黑夜） -->
    <transition name="pop">
      <div v-if="themeOpen" class="pop-mask" @click="themeOpen = false">
        <div class="pop-card theme-card" @click.stop>
          <p class="pop-title">{{ '主题' }}</p>
          <div class="theme-grid">
            <button
              v-for="th in THEME_ORDER"
              :key="th"
              class="theme-opt"
              :class="{ active: theme === th }"
              type="button"
              @click="selectTheme(th); themeOpen = false"
            >
              <span class="theme-swatch" :class="'swatch-' + th"></span>
              <span class="theme-name">{{ themeName(th) }}</span>
            </button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 更多菜单（顶栏 ⋯：简繁 / 书签 / 书签列表 / 搜索本章 / 复制 / 自动阅读） -->
    <transition name="pop">
      <div v-if="moreOpen" class="pop-mask" @click="moreOpen = false">
        <div class="pop-card more-card" @click.stop>
          <p class="pop-title">{{ '更多' }}</p>
          <div class="more-grid">
            <button v-if="isTextBook" class="more-item" type="button" @click="toggleHan; moreOpen = false">
              <svg class="more-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M4 6h16" />
                <path d="M8 6v12" />
                <path d="M16 6v12" />
                <path d="M4 18h8" />
              </svg>
              <span class="more-name">{{ '简繁' }} · {{ hanTargetLabel }}</span>
            </button>
            <button v-if="isTextBook" class="more-item" type="button" @click="addBookmark; moreOpen = false">
              <svg class="more-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M6 4h12v16l-6-4-6 4z" />
              </svg>
              <span class="more-name">{{ '＋书签' }}</span>
            </button>
            <button v-if="isTextBook" class="more-item" type="button" @click="openBookmarks; moreOpen = false">
              <svg class="more-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 4h6l-1 6 3 3v2H7v-2l3-3z" />
                <path d="M12 15v5" />
              </svg>
              <span class="more-name">{{ '书签' }}</span>
            </button>
            <button v-if="isTextBook" class="more-item" type="button" @click="openChapterSearch; moreOpen = false">
              <svg class="more-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="11" cy="11" r="7" />
                <path d="M21 21l-4.35-4.35" />
              </svg>
              <span class="more-name">{{ '搜索' }}</span>
            </button>
            <button v-if="isTextBook" class="more-item" type="button" :disabled="loading || loadError || paragraphs.length === 0" @click="copyChapter; moreOpen = false">
              <svg class="more-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <rect x="8" y="8" width="12" height="12" rx="2" />
                <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
              </svg>
              <span class="more-name">{{ '复制本章' }}</span>
            </button>
            <button v-if="isTextBook" class="more-item" type="button" :class="{ active: autoPlaying }" @click="toggleAuto; moreOpen = false">
              <svg class="more-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M8 5v14l11-7z" />
              </svg>
              <span class="more-name">{{ autoPlaying ? '停止' : '自动' }}</span>
            </button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 自定义主题弹层（第 5 档主题：背景色/文字色/强调色 + 恢复默认；localStorage reader_theme_custom） -->
    <transition name="pop">
      <div v-if="customOpen" class="pop-mask custom-mask" @click="customOpen = false">
        <div class="pop-card" @click.stop>
          <p class="pop-title">自定义主题</p>
          <p class="pop-hint">背景色 / 文字色 / 强调色 · 仅保存在本机</p>
          <div class="custom-row">
            <span class="set-label">背景色</span>
            <input v-model="customTheme.bg" class="custom-color" type="color" title="背景色" />
          </div>
          <div class="custom-row">
            <span class="set-label">文字色</span>
            <input v-model="customTheme.text" class="custom-color" type="color" title="文字色" />
          </div>
          <div class="custom-row">
            <span class="set-label">强调色</span>
            <input v-model="customTheme.accent" class="custom-color" type="color" title="强调色" />
          </div>
          <div class="set-foot">
            <button
              class="text-btn"
              type="button"
              title="恢复默认配色（米白纸色 + 深褐文字 + 棕金强调）"
              @click="resetCustomTheme"
            >
              恢复默认
            </button>
            <button class="pop-btn" type="button" @click="customOpen = false">完成</button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 听书面板（后端语音合成） -->
    <transition name="pop">
      <div v-if="ttsPanelOpen" class="pop-mask" @click="ttsPanelOpen = false">
        <div class="pop-card tts-card" @click.stop>
          <p class="pop-title">听书</p>
          <p class="pop-hint">后端语音合成 · 本章播完自动连播下一章</p>

          <div class="set-row">
            <span class="set-label">引擎</span>
            <div class="seg">
              <button
                class="seg-btn"
                type="button"
                :class="{ active: ttsEngine === 'edge' }"
                @click="ttsEngine = 'edge'"
              >
                Edge
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: ttsEngine === 'http' }"
                :disabled="ttsHttpList.length === 0"
                :title="ttsHttpList.length === 0 ? '未配置 HttpTTS 源（设置页添加）' : ''"
                @click="ttsEngine = 'http'"
              >
                HttpTTS
              </button>
            </div>
          </div>

          <div v-if="ttsEngine === 'http'" class="set-row">
            <span class="set-label">音源</span>
            <select v-model="ttsHttpUrl" class="tts-select">
              <option v-for="t in ttsHttpList" :key="t.url" :value="t.url">{{ t.name }}</option>
            </select>
          </div>

          <div class="set-row">
            <span class="set-label">音色</span>
            <select v-model="ttsVoice" class="tts-select" :title="ttsVoice">
              <optgroup v-for="g in ttsLocaleGroups" :key="g.label" :label="g.label">
                <option v-for="v in g.voices" :key="v.value" :value="v.value">
                  {{ v.name }} · {{ v.gender === 'Female' ? '女声' : '男声' }}
                </option>
              </optgroup>
              <option v-if="ttsVoices.length === 0" :value="ttsVoice">{{ ttsVoice }}</option>
            </select>
          </div>

          <div class="set-row">
            <span class="set-label">语速</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="ttsRate <= 0.5"
                title="减慢"
                @click="ttsRate = round1(Math.max(0.5, ttsRate - 0.1))"
              >
                −
              </button>
              <span class="set-value">{{ ttsRate.toFixed(1) }}</span>
              <button
                class="set-btn"
                type="button"
                :disabled="ttsRate >= 2"
                title="加快"
                @click="ttsRate = round1(Math.min(2, ttsRate + 0.1))"
              >
                ＋
              </button>
            </div>
          </div>

          <div class="set-row">
            <span class="set-label">音调</span>
            <div class="set-controls">
              <button
                class="set-btn"
                type="button"
                :disabled="ttsPitch <= -10"
                title="降低音调"
                @click="ttsPitch = Math.max(-10, ttsPitch - 1)"
              >
                −
              </button>
              <span class="set-value">{{ ttsPitchParam }}</span>
              <button
                class="set-btn"
                type="button"
                :disabled="ttsPitch >= 10"
                title="升高音调"
                @click="ttsPitch = Math.min(10, ttsPitch + 1)"
              >
                ＋
              </button>
            </div>
          </div>

          <div class="tts-controls">
            <button
              class="pop-btn tts-play"
              type="button"
              :disabled="ttsState === 'loading'"
              @click="ttsPlayPause"
            >
              {{ ttsState === 'playing' ? '暂停' : ttsState === 'paused' ? '继续' : ttsState === 'loading' ? '加载中…' : '播放' }}
            </button>
            <button class="tts-stop" type="button" :disabled="ttsState === 'idle'" @click="stopTts">
              停止
            </button>
          </div>
        </div>
      </div>
    </transition>

    <!-- 隐藏音频元素：后端 TTS 音频流播放（ended → 自动连播） -->
    <audio ref="ttsAudioRef" preload="auto" @ended="onTtsEnded" @error="onTtsError"></audio>

    <!-- 音频书播放元素（书源音频流直连；ended → 自动下一章；m3u8 经 hls.js 接管） -->
    <audio
      v-if="isAudioBook"
      ref="mediaAudioRef"
      preload="auto"
      @loadedmetadata="onAudioLoadedMeta"
      @timeupdate="onAudioTimeUpdate"
      @play="onAudioPlay"
      @pause="onAudioPause"
      @waiting="onAudioWaiting"
      @playing="onAudioPlaying"
      @ended="onAudioEnded"
    ></audio>

    <!-- GAP 102：正文图片全屏查看（遮罩 + 原始图 + 关闭；点击遮罩/关闭按钮退出） -->
    <transition name="pop">
      <div v-if="imgViewerOpen" class="img-viewer" @click="closeImgViewer">
        <img :src="imgViewerUrl" class="img-viewer-original" alt="正文图片" @click.stop />
        <button class="img-viewer-close" type="button" title="关闭" @click="closeImgViewer">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>
    </transition>

    <!-- 书签列表弹层 -->
    <transition name="pop">
      <div v-if="bookmarksOpen" class="pop-mask" @click="bookmarksOpen = false">
        <div class="pop-card bookmark-card" @click.stop>
          <p class="pop-title">书签</p>
          <p v-if="bookmarkLoading" class="pop-hint">加载中…</p>
          <p v-else-if="bookmarks.length === 0" class="pop-hint">暂无书签，点顶栏「＋书签」添加</p>
          <ul v-else class="bm-list">
            <li v-for="(b, i) in bookmarks" :key="`${b.title}-${b.createdAt}-${i}`" class="bm-item">
              <button
                type="button"
                class="bm-jump"
                :title="`跳转：${chapterTitleAt(b.chapterIndex)}`"
                @click="jumpToBookmark(b)"
              >
                <span class="bm-chapter">{{ chapterTitleAt(b.chapterIndex) }}</span>
                <span class="bm-text">{{ b.title }}</span>
                <span class="bm-time">{{ fmtBookmarkTime(b.createdAt) }}</span>
              </button>
              <button type="button" class="bm-del" title="删除书签" @click="deleteBookmarkItem(b)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </li>
          </ul>
        </div>
      </div>
    </transition>

    <!-- 划词工具条（复制 / 朗读） -->
    <transition name="pop">
      <div v-if="selOpen" class="sel-bar" :style="{ left: `${selX}px`, top: `${selY}px` }">
        <button type="button" class="sel-btn" @click="copySelection">复制</button>
        <button type="button" class="sel-btn" title="朗读选中文本" @click="speakSelection">朗读</button>
      </div>
    </transition>

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
          <div ref="drawerListRef" class="drawer-list">
            <template v-for="(ch, i) in drawerChapters" :key="`${ch.url}-${i}`">
              <button
                v-if="ch.isVolume"
                type="button"
                class="chapter-volume"
                :class="{ collapsed: tocCollapsed[ch.title] }"
                :title="tocCollapsed[ch.title] ? '展开本卷' : '折叠本卷'"
                @click="toggleVolume(ch.title)"
              >
                <span class="vol-arrow">{{ tocCollapsed[ch.title] ? '▸' : '▾' }}</span>
                <span class="vol-title">{{ ch.title }}</span>
              </button>
              <button
                v-else
                v-show="!chapterHidden[i]"
                type="button"
                class="chapter-item"
                :class="{ current: i === chapterIndex }"
                @click="goToChapter(i)"
              >
                <span class="chapter-item-title">{{ ch.title }}</span>
                <span v-if="ch.wcLabel" class="chapter-item-wc">{{ ch.wcLabel }}</span>
              </button>
            </template>
          </div>
        </aside>
      </div>
    </transition>

    <!-- 翻页模式浮动按钮：上下翻页 ▲/▼ 翻页；左右翻页/仿真翻页 ‹/› 翻页 -->
    <template v-if="isTextBook && !loading && !loadError && paragraphs.length > 0">
      <div v-if="pageMode === 'slide'" class="flip-nav-vert">
        <button class="flip-nav-btn" type="button" :disabled="flipPageIdx <= 0" title="上一页" @click="flipPage(-1)">▲</button>
        <button class="flip-nav-btn" type="button" title="下一页" @click="flipPage(1)">▼</button>
      </div>
      <template v-else-if="pageMode === 'hslide' || pageMode === 'flip'">
        <button
          class="flip-nav-btn flip-nav-side prev"
          type="button"
          :disabled="flipPageIdx <= 0"
          title="上一页"
          @click="flipPage(-1)"
        >
          ‹
        </button>
        <button
          class="flip-nav-btn flip-nav-side next"
          type="button"
          title="下一页"
          @click="flipPage(1)"
        >
          ›
        </button>
      </template>
    </template>
  </div>
</template>

<style scoped>
.reader-page {
  min-height: 100vh;
  /* 阅读主题变量在 .reader-page[data-reader-theme] 上覆盖（与界面主题分离），此处显式取背景 */
  background: var(--bg);
  animation: fade-in 0.2s ease both;
}

/* ================= GAP 5：纸纹（细噪点——radial-gradient 微纹理叠在背景色上；亮色主题黑点、深色主题白点） ================= */
.reader-page.texture {
  background-image:
    radial-gradient(rgba(0, 0, 0, 0.03) 0.7px, transparent 0.9px),
    radial-gradient(rgba(0, 0, 0, 0.018) 0.5px, transparent 0.8px);
  background-size: 4px 4px, 6px 6px;
  background-position: 0 0, 2px 3px;
}
.reader-page.texture[data-reader-theme='dark'] {
  background-image:
    radial-gradient(rgba(255, 255, 255, 0.025) 0.7px, transparent 0.9px),
    radial-gradient(rgba(255, 255, 255, 0.015) 0.5px, transparent 0.8px);
}

/* ================= GAP 149：顶部细进度条（1px 强调色，scroll 比例） ================= */
.reading-progress {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  z-index: 25;
  height: 2px;
  padding: 0;
  border: none;
  background: transparent;
  cursor: pointer;
}
.reading-progress-fill {
  display: block;
  height: 100%;
  background: var(--accent);
  transition: width 0.15s ease;
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
  background: var(--bg-float);
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
.font-btn.tts-btn.active,
.font-btn.auto-btn.active,
.font-btn.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
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
  background: var(--accent-soft);
  filter: brightness(1.06);
}

/* ================= 正文 ================= */
.reader-main {
  width: 100%;
  margin: 0 auto;
  padding: 48px 24px 150px;
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
  text-indent: 2em;
  word-break: break-word;
}
/* 搜索跳转后的短暂高亮 */
.reader-para.flash {
  animation: search-flash 1.6s ease;
}
@keyframes search-flash {
  0%,
  55% {
    background: rgba(255, 193, 7, 0.35);
  }
  100% {
    background: transparent;
  }
}

/* 页内搜索：正文 <mark> 高亮（v-html 注入 → :deep）——普通命中淡黄底，当前命中描边突出 */
.reader-para :deep(mark) {
  background: rgba(255, 193, 7, 0.45);
  color: inherit;
  border-radius: 2px;
  padding: 0 1px;
}
.reader-para :deep(mark.mark-current) {
  background: rgba(255, 170, 0, 0.75);
  box-shadow: 0 0 0 2px rgba(255, 170, 0, 0.5);
  color: #000;
}

/* GAP 7：TTS 朗读中当前段落浅背景高亮 */
.reader-para.tts-reading {
  background: var(--tts-reading-bg, rgba(79, 70, 229, 0.12));
  border-radius: 4px;
  box-shadow: 0 0 0 4px var(--tts-reading-bg, rgba(79, 70, 229, 0.12));
  transition: background 0.25s ease;
}

/* GAP 102：正文图片（段落内单张图片） */
.reader-img {
  display: block;
  max-width: 100%;
  max-height: 72vh;
  margin: 0 auto 1em;
  border-radius: 8px;
  cursor: zoom-in;
  opacity: 0;
  transition: opacity 0.3s ease;
}
.reader-img.is-loaded {
  opacity: 1;
}

/* GAP 102：图片全屏查看层 */
.img-viewer {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 32px;
  background: rgba(10, 10, 12, 0.92);
}
.img-viewer-original {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  border-radius: 4px;
  box-shadow: 0 8px 40px rgba(0, 0, 0, 0.5);
}
.img-viewer-close {
  position: fixed;
  top: 18px;
  right: 18px;
  width: 38px;
  height: 38px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid rgba(255, 255, 255, 0.25);
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.08);
  color: rgba(255, 255, 255, 0.9);
  cursor: pointer;
  transition: background 0.2s ease, color 0.2s ease;
}
.img-viewer-close:hover {
  background: rgba(255, 255, 255, 0.2);
  color: #fff;
}
.img-viewer-close svg {
  width: 16px;
  height: 16px;
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

/* ================= 非文本书（音频/漫画/视频/文件） ================= */
.media-stage {
  padding: 24px 0 8px;
}
.media-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 18px;
  padding: 36px 28px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
}
.media-art {
  position: relative;
  width: 150px;
  height: 150px;
  border-radius: 12px;
  background: var(--border) center/cover no-repeat;
  background-image: linear-gradient(135deg, var(--border-strong), var(--border));
  display: flex;
  align-items: center;
  justify-content: center;
}
.media-badge {
  padding: 5px 14px;
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.45);
  color: #fff;
  font-size: 12px;
  letter-spacing: 2px;
}
.media-buffering {
  position: absolute;
  right: 8px;
  bottom: 8px;
  padding: 3px 10px;
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.5);
  color: #fff;
  font-size: 11px;
}
.media-chapter {
  margin: 0;
  font-size: 15px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--text-1);
  text-align: center;
}
.media-controls {
  display: flex;
  align-items: center;
  gap: 22px;
}
.media-nav {
  height: 36px;
  padding: 0 18px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  letter-spacing: 2px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.media-nav:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.media-nav:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
.media-play {
  width: 62px;
  height: 62px;
  border: none;
  border-radius: 50%;
  background: var(--accent);
  color: var(--on-accent);
  font-size: 20px;
  font-family: inherit;
  cursor: pointer;
  box-shadow: var(--shadow-md);
  transition: transform 0.15s ease;
}
.media-play:hover:not(:disabled) {
  transform: scale(1.06);
}
.media-play:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.media-progress {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  max-width: 520px;
}
.media-time {
  min-width: 44px;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: var(--text-3);
}
.media-slider {
  flex: 1;
  accent-color: var(--accent);
  cursor: pointer;
}
.media-hint {
  margin: 0;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
  text-align: center;
}

/* ---- 漫画书 ---- */
.comic-scroll {
  display: flex;
  overflow-x: auto;
  scroll-snap-type: x mandatory;
  gap: 10px;
  padding: 4px 2px 14px;
  scrollbar-width: thin;
  -webkit-overflow-scrolling: touch;
}
.comic-page {
  position: relative;
  flex: 0 0 min(96vw, 860px);
  scroll-snap-align: center;
  border-radius: 6px;
  overflow: hidden;
  background: var(--border);
  min-height: 220px;
}
.comic-page.active {
  outline: 1px solid var(--accent);
}
.comic-placeholder {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 12px;
  letter-spacing: 2px;
  color: var(--text-3);
  animation: pulse 1.2s ease-in-out infinite;
}
.comic-img {
  position: relative;
  display: block;
  width: 100%;
  height: auto;
  min-height: 220px;
  object-fit: contain;
  cursor: pointer;
}
.comic-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 6px 2px 0;
  flex-wrap: wrap;
}
.comic-hint {
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.comic-page-indicator {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  letter-spacing: 1px;
  color: var(--text-2);
}

/* ---- 视频书 ---- */
.video-player {
  display: block;
  width: 100%;
  max-height: 72vh;
  border-radius: var(--radius);
  background: #000;
}

/* ---- 文件书 ---- */
.file-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 18px;
  padding: 48px 28px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  text-align: center;
}
.file-title {
  margin: 0;
  font-size: 18px;
  font-weight: 500;
  letter-spacing: 1px;
  color: var(--text-1);
}
.file-intro {
  margin: 0;
  max-width: 560px;
  font-size: 13px;
  font-weight: 300;
  line-height: 1.9;
  color: var(--text-2);
  white-space: pre-wrap;
}
.download-btn {
  display: inline-block;
  padding: 12px 44px;
  border-radius: var(--radius);
  background: var(--accent);
  color: var(--on-accent);
  font-size: 14px;
  letter-spacing: 3px;
  text-decoration: none;
  box-shadow: var(--shadow-md);
  transition: transform 0.15s ease;
}
.download-btn:hover {
  transform: scale(1.04);
}

/* ================= 进度条（底部细字） ================= */
/* ================= 底部工具栏（图标+文字上下布局） ================= */
.reader-toolbar {
  position: fixed;
  left: 50%;
  bottom: calc(46px + env(safe-area-inset-bottom));
  transform: translateX(-50%);
  z-index: 29;
  width: min(680px, 100%);
  display: flex;
  align-items: stretch;
  justify-content: space-around;
  gap: 4px;
  padding: 8px 16px 10px;
  border-top: 1px solid var(--border);
  background: var(--bg);
  box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.04);
}
.tb-btn {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 6px 2px 4px;
  border: none;
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease;
}
.tb-btn:hover:not(:disabled) {
  color: var(--text-1);
  background: var(--hover);
}
.tb-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
.tb-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 20px;
  font-size: 13px;
  font-weight: 500;
  line-height: 1;
  color: var(--text-1);
}
.tb-icon svg {
  width: 18px;
  height: 18px;
}
.tb-label {
  max-width: 100%;
  font-size: 10.5px;
  font-weight: 300;
  letter-spacing: 0.5px;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: color 0.2s ease;
}
.tb-btn:hover .tb-label {
  color: var(--text-2);
}
.tb-theme-dot {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.04);
}
.dot-light {
  background: #fafafa;
}
.dot-warm {
  background: #f7f0e6;
}
.dot-green {
  background: #3f8a4f;
}
.dot-dark {
  background: #17171a;
}
.dot-black {
  background: #000000;
  border-color: #3a3a3a;
}

/* ================= 主题预设弹层 ================= */
.theme-card {
  width: min(360px, 100%);
}
.theme-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 8px;
}
.theme-opt {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 7px;
  padding: 12px 4px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  color: var(--text-2);
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    box-shadow 0.2s ease;
}
.theme-opt:hover {
  border-color: var(--border-strong);
}
.theme-opt.active {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
}
.theme-swatch {
  width: 34px;
  height: 34px;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.05);
}
.theme-name {
  font-size: 12px;
}
.theme-opt.active .theme-name {
  color: var(--accent);
}
.swatch-light {
  background: #fafafa;
}
.swatch-warm {
  background: #f7f0e6;
}
.swatch-green {
  background: #3f8a4f;
}
.swatch-dark {
  background: #17171a;
}
.swatch-black {
  background: #000000;
}

/* ================= 更多菜单 ================= */
.more-card {
  width: min(320px, 100%);
}
.more-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 8px;
}
.more-item {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 12px 6px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  color: var(--text-2);
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    color 0.2s ease;
}
.more-item:hover:not(:disabled) {
  border-color: var(--border-strong);
  color: var(--text-1);
}
.more-item:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.more-item.active {
  color: var(--accent);
  border-color: var(--accent);
}
.more-icon {
  width: 20px;
  height: 20px;
}
.more-name {
  font-size: 11.5px;
}
.set-foot-spacer {
  flex: 1;
}
.more-btn svg {
  width: 16px;
  height: 16px;
}

.progress-bar {
  position: fixed;
  left: 50%;
  bottom: 0;
  transform: translateX(-50%);
  z-index: 30;
  width: min(680px, 100%);
  padding: 10px 24px 10px;
  border: none;
  border-top: 1px solid var(--border);
  background: var(--bg);
  box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.04);
  cursor: pointer;
  font-family: inherit;
  text-align: center;
}
.progress-track {
  display: block;
  height: 2px;
  background: var(--border);
  border-radius: 1px;
  overflow: hidden;
}
.progress-fill {
  display: block;
  height: 100%;
  background: var(--accent);
  transition: width 0.2s ease;
}
.progress-text {
  display: block;
  margin-top: 7px;
  font-size: 11px;
  font-weight: 300;
  letter-spacing: 1.5px;
  color: var(--text-3);
  transition: color 0.2s ease;
}
.progress-bar:hover .progress-text {
  color: var(--accent);
}

.seg-row {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.font-picker {
  position: relative;
}
.font-trigger {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  width: 100%;
  padding: 8px 14px;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  cursor: pointer;
  transition: border-color 0.2s ease;
}
.font-trigger:hover {
  border-color: var(--accent);
}
.font-caret {
  font-size: 10px;
  color: var(--text-3);
  transition: transform 0.2s ease;
}
.font-caret.open {
  transform: rotate(180deg);
}
.font-menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  z-index: 50;
  min-width: 300px;
  max-height: 300px;
  overflow-y: auto;
  background: var(--bg-float);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: var(--shadow-lg);
  padding: 4px;
}
.font-item {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
  padding: 9px 12px;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
  background: none;
  border: none;
  border-radius: 6px;
  text-align: left;
  cursor: pointer;
  transition: background 0.15s ease, color 0.15s ease;
}
.font-item-name {
  flex-shrink: 0;
  font-size: 13px;
}
.font-item-demo {
  font-size: 12px;
  color: var(--text-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.font-item:hover {
  background: var(--accent-soft);
  color: var(--accent);
}
.font-item.active {
  color: var(--accent);
  background: var(--accent-soft);
}
.fade-drop-enter-active,
.fade-drop-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.fade-drop-enter-from,
.fade-drop-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
.seg-btn {
  padding: 4px 14px;
  font-size: 12px;
  font-weight: 400;
  color: var(--text-2);
  background: none;
  border: 1px solid var(--border);
  border-radius: 999px;
  cursor: pointer;
  transition: all 0.2s ease;
}
.seg-btn:hover {
  border-color: var(--accent);
  color: var(--accent);
}
.seg-btn.active {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-soft);
}

/* ================= 弹层（设置 / 跳章） ================= */
.pop-mask {
  position: fixed;
  inset: 0;
  z-index: 40;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--overlay);
}
.pop-enter-active,
.pop-leave-active {
  transition: opacity 0.2s ease;
}
.pop-enter-from,
.pop-leave-to {
  opacity: 0;
}
.pop-card {
  width: min(320px, 86vw);
  padding: 26px 26px 20px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--surface);
  /* GAP 6：本书设置 12 行较高——弹层超高时内部滚动 */
  max-height: 84vh;
  overflow-y: auto;
}
.pop-title {
  margin: 0;
  font-size: 14px;
  font-weight: 300;
  letter-spacing: 3px;
  color: var(--text-1);
  text-align: center;
}
.pop-hint {
  margin: 10px 0 0;
  font-size: 11.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
  text-align: center;
}

/* ================= 自定义主题弹层 ================= */
.custom-mask {
  z-index: 45;
}
.custom-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 16px;
}
.custom-color {
  width: 46px;
  height: 28px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: none;
  cursor: pointer;
  transition: border-color 0.2s ease;
}
.custom-color:hover {
  border-color: var(--accent);
}

/* ================= 亮度弹层 ================= */
.bright-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 22px;
}
.bright-slider {
  flex: 1;
  min-width: 0;
  height: 4px;
  accent-color: var(--accent);
  cursor: pointer;
}
.bright-value {
  flex-shrink: 0;
  min-width: 38px;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-1);
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.pop-row {
  display: flex;
  gap: 10px;
  margin-top: 18px;
}
.pop-input {
  flex: 1;
  min-width: 0;
  height: 36px;
  padding: 0 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg);
  color: var(--text-1);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  outline: none;
  transition: border-color 0.2s ease;
}
.pop-input:focus {
  border-color: var(--accent);
}
.pop-input::-webkit-outer-spin-button,
.pop-input::-webkit-inner-spin-button {
  -webkit-appearance: none;
  margin: 0;
}
.pop-btn {
  flex-shrink: 0;
  height: 36px;
  padding: 0 18px;
  border: none;
  border-radius: var(--radius);
  background: var(--accent);
  color: var(--on-accent);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 2px;
  cursor: pointer;
  transition: background 0.2s ease;
}
.pop-btn:hover {
  background: var(--accent-deep);
}

/* ================= 页内搜索弹层 ================= */
.search-card {
  width: min(460px, 92vw);
}
.search-nav {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
}
.search-nav .pop-btn {
  flex: 1;
}

/* ================= 排版设置 ================= */
.cfg-tabs {
  display: flex;
  gap: 6px;
  margin-top: 18px;
  padding: 3px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--bg);
}
.cfg-tab {
  flex: 1;
  height: 28px;
  border: none;
  border-radius: 999px;
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 2px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    background 0.2s ease;
}
.cfg-tab.active {
  color: var(--accent);
  background: var(--accent-soft);
}
.set-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 20px;
}
.set-label {
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-2);
}
.set-controls {
  display: flex;
  align-items: center;
  gap: 10px;
}
.set-btn {
  width: 26px;
  height: 26px;
  border: 1px solid var(--border);
  border-radius: 50%;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 14px;
  line-height: 1;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.set-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.set-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
/* 极简开关（替换规则） */
.switch {
  position: relative;
  flex-shrink: 0;
  width: 36px;
  height: 20px;
  border-radius: 999px;
  border: 1px solid var(--border-strong);
  background: none;
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.switch .switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--text-3);
  transition:
    transform 0.2s ease,
    background-color 0.2s ease;
}
.switch:hover {
  border-color: var(--accent);
}
.switch.on {
  border-color: var(--accent);
  background: var(--accent-soft);
}
.switch.on .switch-knob {
  transform: translateX(16px);
  background: var(--accent);
}
.manage-link {
  padding: 3px 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.manage-link:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.set-value {
  min-width: 34px;
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-1);
  text-align: center;
  font-variant-numeric: tabular-nums;
}
.seg {
  display: flex;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
}
.seg-btn {
  height: 28px;
  padding: 0 14px;
  border: none;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    background 0.2s ease;
}
.seg-btn + .seg-btn {
  border-left: 1px solid var(--border);
}
.seg-btn.active {
  color: var(--accent);
  background: var(--accent-soft);
}
.seg-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
.set-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 24px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
}
.text-btn {
  padding: 0;
  border: none;
  background: none;
  color: var(--text-3);
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.text-btn:hover {
  color: var(--text-1);
}
.text-btn.danger {
  color: var(--text-3);
}
.text-btn.danger:hover {
  color: var(--danger);
}

/* ================= 章节侧栏 ================= */
.drawer-mask {
  position: fixed;
  inset: 0;
  z-index: 50;
  background: var(--overlay);
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
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 20px 20px 8px;
  border: none;
  background: none;
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-3);
  text-align: left;
  cursor: pointer;
  transition: color 0.15s ease;
}
.chapter-volume:hover {
  color: var(--accent);
}
.vol-arrow {
  flex-shrink: 0;
  font-size: 10px;
  letter-spacing: 0;
  transition: transform 0.15s ease;
}
.vol-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.chapter-item {
  display: flex;
  align-items: baseline;
  gap: 8px;
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
  transition:
    color 0.2s ease,
    background 0.2s ease,
    border-color 0.2s ease;
}
.chapter-item-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 章节字数（目录抽屉）：后端返回或本会话已加载章显示，未加载章省略 */
.chapter-item-wc {
  flex-shrink: 0;
  font-size: 11px;
  font-weight: 300;
  letter-spacing: 0.5px;
  font-variant-numeric: tabular-nums;
  color: var(--text-3);
}
.chapter-item:hover {
  color: var(--text-1);
  background: var(--hover);
}
.chapter-item.current {
  color: var(--accent);
  border-left-color: var(--accent);
  background: var(--accent-soft);
  font-weight: 400;
}
.chapter-item.current .chapter-item-wc {
  color: var(--accent);
}

/* ================= 听书面板 ================= */
.tts-card {
  width: min(320px, 86vw);
}
.tts-select {
  width: 150px;
  height: 30px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg);
  color: var(--text-1);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  outline: none;
  cursor: pointer;
  transition: border-color 0.2s ease;
}
.tts-select:focus {
  border-color: var(--accent);
}
.tts-controls {
  display: flex;
  gap: 10px;
  margin-top: 22px;
}
.tts-play {
  flex: 1;
}
.tts-stop {
  flex-shrink: 0;
  height: 36px;
  padding: 0 18px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 2px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.tts-stop:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.tts-stop:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

/* ================= 书签弹层 ================= */
.bookmark-card {
  width: min(380px, 92vw);
}
.bm-list {
  list-style: none;
  margin: 16px 0 0;
  padding: 0;
  max-height: 46vh;
  overflow-y: auto;
}
.bm-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 2px;
  border-bottom: 1px solid var(--border);
}
.bm-item:last-child {
  border-bottom: none;
}
.bm-jump {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  padding: 0;
  border: none;
  background: none;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
}
.bm-chapter {
  max-width: 100%;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--accent);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.bm-text {
  max-width: 100%;
  font-size: 13px;
  font-weight: 300;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.bm-time {
  font-size: 11px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.bm-del {
  flex-shrink: 0;
  width: 26px;
  height: 26px;
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
.bm-del:hover {
  color: var(--danger);
}
.bm-del svg {
  width: 12px;
  height: 12px;
}

/* ================= 划词工具条 ================= */
.sel-bar {
  position: fixed;
  z-index: 60;
  display: flex;
  transform: translateX(-50%);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface);
  box-shadow: 0 4px 18px rgba(0, 0, 0, 0.1);
}
.sel-btn {
  padding: 6px 14px;
  border: none;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.sel-btn + .sel-btn {
  border-left: 1px solid var(--border);
}
.sel-btn:hover {
  color: var(--accent);
}

/* ================= 翻页模式：仿真翻页（横向分页）/ 切章过渡 / 浮动按钮 ================= */
/* 仿真翻页：阅读页锁高，正文区弹性撑满，禁止整页纵向滚动 */
.reader-page.flip-layout {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
}
.reader-page.flip-layout .reader-main {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  /* 底部工具栏 + 进度条（fixed）约 90px：正文分页底部留白避免被遮挡 */
  padding-bottom: 96px;
}
.reader-page.flip-layout .chapter-title,
.reader-page.flip-layout .chapter-nav {
  flex-shrink: 0;
}
.flip-view {
  width: 100%;
}
.flip-view.active {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.flip-view.active .reader-content {
  height: 100%;
  column-fill: auto;
}
.flip-view.active .reader-img {
  break-inside: avoid;
}
/* hslide 切章过渡：正文随方向滑入（翻页动画） */
.chapter-slide-in-right {
  animation: chapter-slide-in-right 0.32s ease both;
}
.chapter-slide-in-left {
  animation: chapter-slide-in-left 0.32s ease both;
}
@keyframes chapter-slide-in-right {
  from {
    opacity: 0.25;
    transform: translateX(48px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@keyframes chapter-slide-in-left {
  from {
    opacity: 0.25;
    transform: translateX(-48px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
/* 翻页模式浮动按钮 */
.flip-nav-vert {
  position: fixed;
  right: 10px;
  top: 50%;
  transform: translateY(-50%);
  z-index: 15;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.flip-nav-btn {
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border);
  border-radius: 50%;
  background: var(--surface);
  color: var(--text-2);
  font-family: inherit;
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
  box-shadow: var(--shadow-sm);
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    opacity 0.2s ease;
}
.flip-nav-btn:hover:not(:disabled) {
  color: var(--accent);
  border-color: var(--accent);
}
.flip-nav-btn:disabled {
  opacity: 0.35;
  cursor: default;
}
.flip-nav-side {
  position: fixed;
  top: 50%;
  transform: translateY(-50%);
  z-index: 15;
}
.flip-nav-side.prev {
  left: 8px;
}
.flip-nav-side.next {
  right: 8px;
}
/* 翻页模式 4 按钮塞入 320px 弹层：收紧内边距 */
.seg.compact .seg-btn {
  padding: 0 9px;
  font-size: 11.5px;
  letter-spacing: 0.5px;
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    flex-wrap: wrap;
    padding: 8px 10px;
    gap: 8px;
  }
  .icon-btn {
    width: 32px;
    height: 32px;
  }
  .book-name {
    order: 2;
    flex: 1;
    font-size: 13px;
  }
  /* 顶部操作栏：第二行整宽，横向滚动（触屏滑动），按钮不压缩 */
  .top-actions {
    order: 3;
    width: 100%;
    gap: 6px;
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
    scrollbar-width: none;
    padding-bottom: 2px;
  }
  .top-actions::-webkit-scrollbar {
    display: none;
  }
  .top-actions .font-btn,
  .top-actions .toc-btn {
    flex-shrink: 0;
    min-width: 32px;
  }
  .font-btn {
    padding: 0 8px;
  }
  .toc-btn {
    padding: 0 10px;
    margin-left: 0;
  }
  .reader-main {
    padding: 32px 16px 130px;
  }
  .chapter-nav {
    gap: 12px;
  }
  .progress-bar {
    padding: 0 12px 10px;
    padding-bottom: max(10px, env(safe-area-inset-bottom));
  }
}
</style>
