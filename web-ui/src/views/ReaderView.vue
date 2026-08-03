<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { getBookshelf, deleteBook } from '@/api/bookshelf'
import { getBookInfo, getBookToc, getBookContent } from '@/api/books'
import { get, post } from '@/api/request'
import { loadReplaceRules } from '@/api/replaceRules'
import { simplized, traditionalized } from '@/utils/chinese'
import type { Book, BookChapter, Bookmark, ReplaceRule } from '@/types'

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

/* ---------------- 1. 主题（浅色/深色/纸色/跟随系统） ---------------- */

type Theme = 'light' | 'dark' | 'paper' | 'system'
const THEME_KEY = 'reader_theme'
const THEME_ORDER: Theme[] = ['light', 'dark', 'paper', 'system']
const THEME_LABEL: Record<Theme, string> = { light: '浅', dark: '深', paper: '纸', system: '系统' }
const theme = ref<Theme>('light')
{
  const raw = localStorage.getItem(THEME_KEY)
  if (raw === 'light' || raw === 'dark' || raw === 'paper' || raw === 'system') theme.value = raw
}
/** 系统深色偏好（theme=system 时生效；matchMedia 监听切换） */
const systemDark = ref(false)
let mediaQuery: MediaQueryList | null = null
const systemTheme = (): Theme => (systemDark.value ? 'dark' : 'light')
function applyTheme(t: Theme) {
  document.documentElement.dataset.theme = t === 'system' ? systemTheme() : t
}
function onSystemThemeChange(e: MediaQueryListEvent) {
  systemDark.value = e.matches
  if (theme.value === 'system') applyTheme(theme.value)
}
watch(theme, (t) => {
  applyTheme(t)
  persist(THEME_KEY, t)
})
function cycleTheme() {
  const i = THEME_ORDER.indexOf(theme.value)
  theme.value = THEME_ORDER[(i + 1) % THEME_ORDER.length]
}

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
contentWidth.value = localStorage.getItem('reader_content_width') ?? WIDTH_OPTIONS[1].value
function setWidth(v: string) {
  contentWidth.value = v
  localStorage.setItem('reader_content_width', v)
}
const paraSpacing = ref(1)
const fontWeight = ref(400)
{
  const raw = Number(localStorage.getItem(FONT_KEY))
  if (raw >= MIN_FONT && raw <= MAX_FONT) fontSize.value = raw
  lineHeight.value = loadSetting('reader_line_height', MIN_LINE, MAX_LINE, 1.9, 0.1)
  paraSpacing.value = loadSetting('reader_para_spacing', MIN_PARA, MAX_PARA, 1, 0.1)
  fontWeight.value = loadSetting('reader_font_weight', MIN_WEIGHT, MAX_WEIGHT, 400, 50)
}
watch(fontSize, (v) => persist(FONT_KEY, v))
watch(lineHeight, (v) => persist('reader_line_height', v))
watch(paraSpacing, (v) => persist('reader_para_spacing', v))
watch(fontWeight, (v) => persist('reader_font_weight', v))

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
{
  const raw = localStorage.getItem('reader_font_family')
  if (FONT_OPTIONS.some((o) => o.value === raw)) fontKind.value = raw as FontKind
}
watch(fontKind, (v) => persist('reader_font_family', v))
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
letterSpacing.value = loadSetting('reader_letter_spacing', 0, 2, 0, 0.5)
watch(letterSpacing, (v) => persist('reader_letter_spacing', v))

const textIndent = ref(true)
{
  const raw = localStorage.getItem('reader_text_indent')
  if (raw === '0') textIndent.value = false
}
watch(textIndent, (v) => persist('reader_text_indent', v ? '1' : '0'))

const textAlign = ref<'left' | 'justify'>('left')
{
  const raw = localStorage.getItem('reader_text_align')
  if (raw === 'left' || raw === 'justify') textAlign.value = raw
}
watch(textAlign, (v) => persist('reader_text_align', v))

const settingsOpen = ref(false)
function resetTypography() {
  fontSize.value = 18
  lineHeight.value = 1.9
  paraSpacing.value = 1
  fontWeight.value = 400
  fontKind.value = 'system'
  letterSpacing.value = 0
  textIndent.value = true
  textAlign.value = 'left'
}

/* ---------------- 3. 翻页模式（滚动 / 滑动=整页节流） ---------------- */

type PageMode = 'scroll' | 'slide'
const pageMode = ref<PageMode>('scroll')
{
  const raw = localStorage.getItem('reader_page_mode')
  if (raw === 'scroll' || raw === 'slide') pageMode.value = raw
}
watch(pageMode, (m) => persist('reader_page_mode', m))

let slideAcc = 0
let slideCooldown = false
let touchStartY = 0
const SLIDE_THRESHOLD = 48
const SLIDE_PAGE = 0.9

function slideFlip(dir: 1 | -1) {
  if (slideCooldown) return
  slideCooldown = true
  window.setTimeout(() => {
    slideCooldown = false
  }, 420)
  window.scrollBy({ top: dir * window.innerHeight * SLIDE_PAGE, behavior: 'smooth' })
}
function isInsideOverlay(el: EventTarget | null): boolean {
  return el instanceof HTMLElement && !!el.closest('.drawer-mask, .pop-mask, .sel-bar')
}
function onWheel(e: WheelEvent) {
  if (pageMode.value !== 'slide' || isInsideOverlay(e.target)) return
  e.preventDefault()
  slideAcc += e.deltaY
  if (slideAcc >= SLIDE_THRESHOLD) {
    slideAcc = 0
    slideFlip(1)
  } else if (slideAcc <= -SLIDE_THRESHOLD) {
    slideAcc = 0
    slideFlip(-1)
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
  if (dy >= SLIDE_THRESHOLD) slideFlip(1)
  else if (dy <= -SLIDE_THRESHOLD) slideFlip(-1)
}
watch(pageMode, (mode, old) => {
  if (mode === 'slide') {
    window.addEventListener('wheel', onWheel, { passive: false })
    window.addEventListener('touchstart', onTouchStart, { passive: true })
    window.addEventListener('touchmove', onTouchMove, { passive: false })
    window.addEventListener('touchend', onTouchEnd, { passive: true })
  } else if (old === 'slide') {
    window.removeEventListener('wheel', onWheel)
    window.removeEventListener('touchstart', onTouchStart)
    window.removeEventListener('touchmove', onTouchMove)
    window.removeEventListener('touchend', onTouchEnd)
  }
})

/* ---------------- 4. 进度显示 + 章节跳转 ---------------- */

const scrollFrac = ref(0)
const jumpOpen = ref(false)
const jumpNum = ref('')

function updateScrollFrac() {
  const max = document.documentElement.scrollHeight - window.innerHeight
  scrollFrac.value = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0
}
const progressPct = computed(() => {
  const n = realChapters.value.length
  if (!n) return 0
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
  const top = p ? Math.max(0, p.getBoundingClientRect().top + window.scrollY - 64) : 0
  window.scrollTo(0, top)
}

/* ---------------- 4.2 章节内搜索（前端本地：当前章段落包含匹配 + 高亮 + 点击跳段） ---------------- */

const searchOpen = ref(false)
const searchKeyword = ref('')
const searchSearched = ref(false)
/** 搜索结果：段落序号 + 高亮 HTML 片段 */
const searchResults = ref<{ idx: number; html: string }[]>([])
/** 跳转后短暂高亮的目标段落 */
const flashParaIdx = ref(-1)
const searchInputRef = ref<HTMLInputElement | null>(null)
let flashTimer: number | undefined
const SEARCH_MAX_HITS = 100

function openChapterSearch() {
  searchKeyword.value = ''
  searchSearched.value = false
  searchResults.value = []
  searchOpen.value = true
  void nextTick(() => searchInputRef.value?.focus())
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

/** 生成高亮 HTML：对转义后的段落按关键词（大小写不敏感）包 <mark> */
function highlightSnippet(text: string, kw: string): string {
  const esc = escapeHtml(text)
  const escKw = escapeHtml(kw)
  if (!escKw) return esc
  const lower = esc.toLowerCase()
  const kwLower = escKw.toLowerCase()
  const parts: string[] = []
  let from = 0
  let found = 0
  let i = lower.indexOf(kwLower, from)
  while (i !== -1 && found < 60) {
    parts.push(esc.slice(from, i), `<mark>${esc.slice(i, i + escKw.length)}</mark>`)
    from = i + escKw.length
    i = lower.indexOf(kwLower, from)
    found++
  }
  parts.push(esc.slice(from))
  return parts.join('')
}

/** 在当前章段落中查找包含关键词的段落（大小写不敏感） */
function runChapterSearch() {
  const kw = searchKeyword.value.trim()
  searchSearched.value = true
  searchResults.value = []
  if (!kw) return
  const lower = kw.toLowerCase()
  const hits: { idx: number; html: string }[] = []
  paragraphs.value.forEach((p, i) => {
    if (hits.length >= SEARCH_MAX_HITS) return
    if (p.toLowerCase().includes(lower)) hits.push({ idx: i, html: highlightSnippet(p, kw) })
  })
  searchResults.value = hits
}

/** 点击结果：跳转该段并短暂高亮 */
function jumpToSearchResult(idx: number) {
  searchOpen.value = false
  searchKeyword.value = ''
  searchSearched.value = false
  searchResults.value = []
  scrollToParagraph(idx)
  flashParaIdx.value = idx
  window.clearTimeout(flashTimer)
  flashTimer = window.setTimeout(() => {
    flashParaIdx.value = -1
  }, 1600)
}

/* ---------------- 5. 简繁转换（legacy chinese.js 移植） ---------------- */

const HAN_KEY = 'reader_han_trad'
const SIMP_MARKS = '们这个发现时说话书电视经过间题长门开关动华红绿线纸级结给变边还进种'
const TRAD_MARKS = '們這這個發現時說話書電視經過間題長門開關動華紅綠線紙級結給變邊還進種'
/** 未手动选择时按正文自动判定简/繁 */
const hanTrad = ref(false)
const hanAuto = ref(true)
{
  const raw = localStorage.getItem(HAN_KEY)
  if (raw === '1') {
    hanTrad.value = true
    hanAuto.value = false
  } else if (raw === '0') {
    hanAuto.value = false
  }
}
watch(hanTrad, (v) => {
  if (!hanAuto.value) persist(HAN_KEY, v ? '1' : '0')
})
function detectTraditional(text: string): boolean {
  let s = 0
  let t = 0
  let n = 0
  for (const ch of text) {
    if (n >= 1500) break
    if (SIMP_MARKS.includes(ch)) {
      s++
      n++
    } else if (TRAD_MARKS.includes(ch)) {
      t++
      n++
    }
  }
  return t > 0 && t > s
}
watch(content, (c) => {
  if (hanAuto.value) hanTrad.value = detectTraditional(c)
})
function toggleHan() {
  hanTrad.value = !hanTrad.value
  hanAuto.value = false
  persist(HAN_KEY, hanTrad.value ? '1' : '0')
}
const hanConvert = (text: string) => (hanTrad.value ? traditionalized(text) : simplized(text))
const hanTargetLabel = computed(() => (hanTrad.value ? '简' : '繁'))

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

/* ---------------- 7. 听书（浏览器 SpeechSynthesis 极简实现；HttpTTS 源见设置页，后端就绪后接入） ---------------- */

const ttsSupported = typeof window !== 'undefined' && 'speechSynthesis' in window
const ttsPlaying = ref(false)
let ttsVoice: SpeechSynthesisVoice | null = null

function pickZhVoice(): SpeechSynthesisVoice | null {
  const voices = window.speechSynthesis.getVoices()
  const zh = voices.filter((v) => v.lang.toLowerCase().startsWith('zh'))
  if (zh.length === 0) return null
  return zh.find((v) => v.lang.toLowerCase() === 'zh-cn') ?? zh[0]
}

if (ttsSupported) {
  // voices 异步加载：首次 getVoices 可能为空，监听 onvoiceschanged 补充
  ttsVoice = pickZhVoice()
  window.speechSynthesis.onvoiceschanged = () => {
    ttsVoice = pickZhVoice()
  }
}

function stopTts() {
  if (!ttsSupported) return
  window.speechSynthesis.cancel()
  ttsPlaying.value = false
}

function startTts() {
  if (!ttsSupported) {
    ElMessage.info('当前浏览器不支持语音朗读')
    return
  }
  const text = paragraphs.value.join('。')
  if (!text) {
    ElMessage.info('本章暂无内容可朗读')
    return
  }
  // Chrome 兼容：cancel 后立即 speak 可能被吞，稍作延迟再播
  window.speechSynthesis.cancel()
  window.setTimeout(() => {
    if (!ttsVoice) ttsVoice = pickZhVoice()
    const u = new SpeechSynthesisUtterance(text)
    u.lang = ttsVoice?.lang ?? 'zh-CN'
    if (ttsVoice) u.voice = ttsVoice
    u.rate = 1
    u.onend = () => {
      ttsPlaying.value = false
    }
    u.onerror = () => {
      ttsPlaying.value = false
    }
    window.speechSynthesis.speak(u)
    ttsPlaying.value = true
  }, 60)
}

function toggleTts() {
  if (ttsPlaying.value) stopTts()
  else startTts()
}

/** 切换章节 / 离开页面时停止朗读 */
watch(chapterIndex, () => stopTts())

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
  const doc = document.documentElement
  if (doc.scrollHeight - window.innerHeight <= 0) return
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

function searchSelection() {
  const text = selText.value
  clearSelection()
  hideSelBar()
  if (!text) return
  void router.push({ path: '/search', query: { key: text } })
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

/** 当前章渲染后调用：静默拉取下一章正文并预热前 5 张图片（仅当含图片 URL） */
function preloadNextChapterImages() {
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
const displayBookName = computed(() => hanConvert(bookName.value))
const displayChapterTitle = computed(() => (currentChapter.value ? hanConvert(currentChapter.value.title) : ''))
/** 目录项（含卷标题）统一按当前简繁模式转换 */
const drawerChapters = computed(() =>
  chapters.value.map((c) => ({ ...c, title: hanConvert(c.title) })),
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
  syncServerProgress()
}

/** 进度服务端同步（POST /reader3/saveBookProgress；失败静默，不影响本地阅读） */
function syncServerProgress() {
  if (!shelfBook.value || !currentChapter.value) return
  void post('/saveBookProgress', {
    bookUrl: bookUrl.value,
    durChapterIndex: chapterIndex.value,
    durChapterPos: Math.round(window.scrollY),
    durChapterTime: Date.now(),
    durChapterTitle: currentChapter.value.title,
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
  } catch {
    loadError.value = true
    return
  } finally {
    loading.value = false
  }
  // 等正文真正渲染（loading 置 false 后）再滚动，避免被加载态高度钳制
  await nextTick()
  if (restoreParagraphIdx != null) {
    await applyRestoreParagraph()
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

/* ---------------- B4 修复：阅读页移出书架入口 ---------------- */

const removing = ref(false)
let removeTimer: number | undefined

async function removeFromShelf() {
  if (!removing.value) {
    removing.value = true
    removeTimer = window.setTimeout(() => {
      removing.value = false
    }, 3000)
    return
  }
  window.clearTimeout(removeTimer)
  removing.value = false
  try {
    await deleteBook(bookUrl.value)
    // request.ts 拦截器已处理失败提示；走到这里即成功
    try {
      localStorage.removeItem(progressKey())
    } catch {
      /* ignore */
    }
    ElMessage.success('已移出书架')
    void router.replace('/')
  } catch {
    /* 已提示 */
  }
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
      notFound.value = true
      return
    }
    // 本地书 tocUrl 可能为空——用 bookUrl 兜底
    if (!found.tocUrl) found.tocUrl = found.bookUrl
    shelfBook.value = found
    bookName.value = found.name
    // 替换规则：进入阅读页时读取一次（localStorage 占位；后端就绪后走 GET /reader3/getReplaceRules）
    if (replaceEnabled.value) refreshReplaceRules()

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

    // 起始章节：服务端进度优先（getBookshelf 已带 durChapter*；durChapterTime>0 才算存过），
    // 无服务端进度则回退 localStorage
    let startIndex = realChapters.value.length ? chapters.value.indexOf(realChapters.value[0]) : 0
    const srvIdx = found.durChapterIndex ?? -1
    const serverSaved =
      typeof srvIdx === 'number' &&
      srvIdx >= 0 &&
      srvIdx < chapters.value.length &&
      !chapters.value[srvIdx].isVolume &&
      (found.durChapterTime ?? 0) > 0
    if (serverSaved) {
      startIndex = srvIdx
      const srvPos = found.durChapterPos ?? 0
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

/* ---------------- B2 修复：目录抽屉打开时定位当前章 ---------------- */

const drawerListRef = ref<HTMLElement | null>(null)
watch(drawerOpen, async (open) => {
  if (!open) return
  await nextTick()
  await nextTick()
  const list = drawerListRef.value
  const el = list?.querySelector<HTMLElement>('.chapter-item.active')
  if (list && el) {
    // 手动计算 scrollTop（不用 scrollIntoView，避免带动页面滚动）
    list.scrollTop = el.offsetTop - list.offsetTop - (list.clientHeight - el.clientHeight) / 2
  }
})

/* ---------------- 生命周期 ---------------- */

function onScroll() {
  updateScrollFrac()
  hideSelBar()
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
  window.addEventListener('resize', updateScrollFrac, { passive: true })
  window.addEventListener('mouseup', onSelectionUp)
  window.addEventListener('touchend', onSelectionUp, { passive: true })
  window.addEventListener('mousedown', onDocMouseDown)
  if (pageMode.value === 'slide') {
    window.addEventListener('wheel', onWheel, { passive: false })
    window.addEventListener('touchstart', onTouchStart, { passive: true })
    window.addEventListener('touchmove', onTouchMove, { passive: false })
    window.addEventListener('touchend', onTouchEnd, { passive: true })
  }
  void init()
})

onBeforeUnmount(() => {
  mediaQuery?.removeEventListener('change', onSystemThemeChange)
  window.removeEventListener('scroll', onScroll)
  window.removeEventListener('resize', updateScrollFrac)
  window.removeEventListener('wheel', onWheel)
  window.removeEventListener('touchstart', onTouchStart)
  window.removeEventListener('touchmove', onTouchMove)
  window.removeEventListener('touchend', onTouchEnd)
  window.removeEventListener('mouseup', onSelectionUp)
  window.removeEventListener('touchend', onSelectionUp)
  window.removeEventListener('mousedown', onDocMouseDown)
  window.clearTimeout(saveTimer)
  window.clearTimeout(removeTimer)
  window.clearTimeout(flashTimer)
  stopTts()
  stopAuto()
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

      <span class="book-name" :title="displayBookName">{{ displayBookName || '阅读' }}</span>

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
        <button
          class="font-btn"
          type="button"
          :title="hanTrad ? '当前繁体，点击转为简体' : '当前简体，点击转为繁体'"
          @click="toggleHan"
        >
          {{ hanTargetLabel }}
        </button>
        <button class="font-btn" type="button" :title="`主题：${theme}（点击切换）`" @click="cycleTheme">
          {{ THEME_LABEL[theme] }}
        </button>
        <button
          class="font-btn tts-btn"
          type="button"
          :class="{ active: ttsPlaying }"
          :title="ttsPlaying ? '停止朗读' : '听书（浏览器语音朗读本章）'"
          :disabled="!ttsSupported"
          @click="toggleTts"
        >
          {{ ttsPlaying ? '停止' : '听书' }}
        </button>
        <button class="font-btn" type="button" title="在当前位置添加书签" @click="addBookmark">
          ＋书签
        </button>
        <button class="font-btn" type="button" title="书签列表" @click="openBookmarks">
          书签
        </button>
        <button class="font-btn" type="button" title="搜索本章内容" @click="openChapterSearch">
          搜索
        </button>
        <button
          class="font-btn auto-btn"
          type="button"
          :class="{ active: autoPlaying }"
          :title="autoPlaying ? '停止自动阅读' : '自动阅读（定时滚动，到底自动切章）'"
          @click="toggleAuto"
        >
          {{ autoPlaying ? '停止' : '自动' }}
        </button>
        <button class="font-btn" type="button" title="排版设置" @click="settingsOpen = true">
          排版
        </button>
        <button class="toc-btn" type="button" title="目录" @click="drawerOpen = true">
          目录
        </button>
      </div>
    </header>

    <!-- 正文 -->
    <main class="reader-main" :style="{ maxWidth: contentWidth }">
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
        <h1 v-if="currentChapter" class="chapter-title">{{ displayChapterTitle }}</h1>

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
        <article
          v-else
          class="reader-content"
          :style="{
            fontSize: `${fontSize}px`,
            lineHeight: `${lineHeight}`,
            fontWeight: `${fontWeight}`,
            fontFamily: fontFamilyStyle || undefined,
            letterSpacing: letterSpacing > 0 ? `${letterSpacing}px` : undefined,
            textAlign,
          }"
        >
          <p
            v-for="(para, i) in paragraphs"
            :key="i"
            class="reader-para"
            :class="{ flash: flashParaIdx === i }"
            :style="{ marginBottom: `${paraSpacing}em`, textIndent: textIndent ? '2em' : '0' }"
          >
            {{ para }}
          </p>
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

    <!-- 进度：底部细字 + 可点击跳章 -->
    <button
      v-if="!loading && !loadError && !notFound && realChapters.length > 0"
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

    <!-- 章节内搜索弹层（本地搜索当前章，段落高亮 + 点击跳段） -->
    <transition name="pop">
      <div v-if="searchOpen" class="pop-mask" @click="searchOpen = false">
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
            />
            <button class="pop-btn" type="button" @click="runChapterSearch">搜索</button>
          </div>
          <p v-if="searchSearched && !searchKeyword.trim()" class="pop-hint">请输入关键词</p>
          <p v-else-if="searchSearched && searchResults.length === 0" class="pop-hint">
            本章未找到「{{ searchKeyword.trim() }}」
          </p>
          <ul v-else-if="searchResults.length" class="search-list">
            <li v-for="r in searchResults" :key="r.idx" class="search-item">
              <button type="button" class="search-jump" :title="`跳转到第 ${r.idx + 1} 段`" @click="jumpToSearchResult(r.idx)">
                <span class="search-idx">{{ r.idx + 1 }}</span>
                <span class="search-text" v-html="r.html"></span>
              </button>
            </li>
          </ul>
          <p v-else class="pop-hint">搜索范围：当前章（前端本地匹配，高亮显示，点击跳转）</p>
        </div>
      </div>
    </transition>

    <!-- 排版设置弹层 -->
    <transition name="pop">
      <div v-if="settingsOpen" class="pop-mask" @click="settingsOpen = false">
        <div class="pop-card" @click.stop>
          <p class="pop-title">排版设置</p>

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

          <div class="set-row">
            <span class="set-label">翻页</span>
            <div class="seg">
              <button
                class="seg-btn"
                type="button"
                :class="{ active: pageMode === 'scroll' }"
                @click="pageMode = 'scroll'"
              >
                滚动
              </button>
              <button
                class="seg-btn"
                type="button"
                :class="{ active: pageMode === 'slide' }"
                @click="pageMode = 'slide'"
              >
                滑动
              </button>
            </div>
          </div>

          <div class="set-row">
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

          <div class="set-row">
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
            <button class="text-btn" type="button" @click="resetTypography">恢复默认</button>
            <button
              class="text-btn danger"
              type="button"
              :title="removing ? '再次点击确认移出书架' : '将本书移出书架'"
              @click="removeFromShelf"
            >
              {{ removing ? '确认移出？' : '移出书架' }}
            </button>
          </div>
        </div>
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

    <!-- 划词工具条（复制 / 搜索） -->
    <transition name="pop">
      <div v-if="selOpen" class="sel-bar" :style="{ left: `${selX}px`, top: `${selY}px` }">
        <button type="button" class="sel-btn" @click="copySelection">复制</button>
        <button type="button" class="sel-btn" @click="searchSelection">搜索</button>
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
.font-btn.auto-btn.active {
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

/* ================= 进度条（底部细字） ================= */
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
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.08);
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
  background: rgba(24, 24, 27, 0.28);
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
  overflow: visible;
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
  color: #fff;
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

/* ================= 章节内搜索弹层 ================= */
.search-card {
  width: min(460px, 92vw);
}
.search-list {
  list-style: none;
  margin: 14px 0 0;
  padding: 0;
  max-height: 46vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.search-item {
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--bg);
  overflow: hidden;
}
.search-jump {
  width: 100%;
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 9px 12px;
  border: none;
  background: none;
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: background 0.15s ease;
}
.search-jump:hover {
  background: var(--accent-soft);
}
.search-idx {
  flex-shrink: 0;
  margin-top: 2px;
  min-width: 20px;
  height: 18px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 4px;
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 11px;
  font-weight: 400;
  font-variant-numeric: tabular-nums;
}
.search-text {
  flex: 1;
  min-width: 0;
  font-size: 12.5px;
  font-weight: 300;
  line-height: 1.7;
  color: var(--text-1);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-word;
}
/* v-html 注入的 <mark> 需要 :deep 才能命中 scoped 样式 */
.search-text :deep(mark) {
  background: rgba(255, 193, 7, 0.4);
  color: inherit;
  border-radius: 2px;
  padding: 0 1px;
}

/* ================= 排版设置 ================= */
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
  color: #cf4444;
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
  background: var(--hover);
}
.chapter-item.active {
  color: var(--accent);
  border-left-color: var(--accent);
  background: var(--accent-soft);
  font-weight: 400;
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
  color: #cf4444;
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

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    padding: 10px 12px;
    gap: 10px;
  }
  .top-actions {
    gap: 6px;
  }
  .font-btn {
    min-width: 30px;
    padding: 0 6px;
  }
  .toc-btn {
    padding: 0 10px;
    margin-left: 0;
  }
  .reader-main {
    padding: 36px 20px 130px;
  }
  .chapter-nav {
    gap: 12px;
  }
  .progress-bar {
    padding: 0 16px 8px;
  }
}
</style>
