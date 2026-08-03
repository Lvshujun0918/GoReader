import { get } from './request'
import { useUserStore } from '@/stores/user'
import type { ReturnData, SystemInfo } from '@/types'

/**
 * 系统信息 + 书源导出
 *
 * GET /reader3/getSystemInfo      → ReturnData<SystemInfo>（版本/端口/用户数/书数/书源数）
 * GET /reader3/exportBookSources  → 当前命名空间书源 JSON 附件下载（attachment）
 */

/** GET /reader3/getSystemInfo */
export function getSystemInfo(): Promise<ReturnData<SystemInfo>> {
  return get<SystemInfo>('/getSystemInfo')
}

/** GET /reader3/exportBookSources：直接触发浏览器下载（后端返回 attachment） */
export function exportBookSources(): void {
  const store = useUserStore()
  const params = new URLSearchParams()
  if (store.accessToken) {
    params.set('accessToken', store.accessToken)
  }
  const url = `/reader3/exportBookSources?${params.toString()}`
  const a = document.createElement('a')
  a.href = url
  a.download = 'bookSource.json'
  document.body.appendChild(a)
  a.click()
  a.remove()
}
