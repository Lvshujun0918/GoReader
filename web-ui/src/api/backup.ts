import { post } from './request'
import { downloadFile } from './file'
import type { ReturnData } from '@/types'

/**
 * POST /reader3/backupToWebdav：备份数据到 WebDAV legado 目录。
 * 响应：ReturnData<{ path: string }>，path 为备份 zip 的绝对路径
 * （storage/data/{ns}/webdav/legado/backup-{ts}.zip）。
 */
export function backupToWebdav(): Promise<ReturnData<{ path: string }>> {
  return post<{ path: string }>('/backupToWebdav')
}

/**
 * 下载备份 zip：backupToWebdav 返回绝对路径，取其文件名，按「用户数据根（__HOME__）下
 * webdav/legado/」的相对路径走 GET /reader3/file/download（file/download 的 path 是 home 根下相对路径）。
 */
export function downloadBackupZip(absPath: string): Promise<Blob> {
  const name = absPath.split(/[\\/]/).filter(Boolean).pop() || 'backup.zip'
  return downloadFile(`webdav/legado/${name}`, '__HOME__')
}
