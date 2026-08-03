import { post } from './request'
import type { ReturnData } from '@/types'

/**
 * POST /reader3/backupToWebdav：备份数据到 WebDAV legado 目录。
 * 响应：ReturnData<{ path: string }>，path 为备份文件路径。
 */
export function backupToWebdav(): Promise<ReturnData<{ path: string }>> {
  return post<{ path: string }>('/backupToWebdav')
}
