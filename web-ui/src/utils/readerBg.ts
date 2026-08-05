/**
 * GAP 4：阅读背景（纯色 / 纸纹 / 图片）。
 * localStorage：reader_bg_mode（color/texture/image）、reader_bg_image（相对用户根路径）。
 * 背景图上传到服务器 assets/background/（file/upload，home=用户根），本地只记路径；
 * 展示时经 file/download 拉取（附 accessToken，BookshelfView 封面同款方式）。
 */

export type BgMode = 'color' | 'texture' | 'image'

export const BG_MODE_KEY = 'reader_bg_mode'
export const BG_IMAGE_KEY = 'reader_bg_image'

export function loadBgMode(): BgMode {
  try {
    const raw = localStorage.getItem(BG_MODE_KEY)
    if (raw === 'texture' || raw === 'image') return raw
  } catch {
    /* ignore */
  }
  return 'color'
}

export function saveBgMode(mode: BgMode) {
  try {
    localStorage.setItem(BG_MODE_KEY, mode)
  } catch {
    /* ignore */
  }
}

export function loadBgImagePath(): string {
  try {
    return localStorage.getItem(BG_IMAGE_KEY) ?? ''
  } catch {
    return ''
  }
}

export function saveBgImagePath(path: string) {
  try {
    if (path) localStorage.setItem(BG_IMAGE_KEY, path)
    else localStorage.removeItem(BG_IMAGE_KEY)
  } catch {
    /* ignore */
  }
}

/** 背景图展示 URL：file/download + accessToken（path 相对用户根；空路径返回 ''） */
export function bgImageUrl(path: string, accessToken: string): string {
  if (!path) return ''
  const base = `/reader3/file/download?path=${encodeURIComponent(path)}`
  return accessToken ? `${base}&accessToken=${encodeURIComponent(accessToken)}` : base
}
