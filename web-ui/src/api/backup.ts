import { post } from './request'
import { downloadFile } from './file'
import type { ReturnData } from '@/types'

/**
 * POST /reader3/backupToWebdav：备份数据到 WebDAV。
 * body { path?: string }：目标子目录（默认 webdav/legado）。
 * GAP 151：路径参数已随请求发送；后端当前固定写入 webdav/legado（create_backup_zip 硬编码），
 * 尚未消费 path 参数——前端先传参预留，后端支持后即可切换目录。
 * 响应：ReturnData<{ path: string }>，path 为备份 zip 的绝对路径
 * （storage/data/{ns}/webdav/legado/backup-{ts}.zip）。
 */
export function backupToWebdav(path?: string): Promise<ReturnData<{ path: string }>> {
  return post<{ path: string }>('/backupToWebdav', path ? { path } : undefined)
}

/**
 * 下载备份 zip：backupToWebdav 返回绝对路径，取其文件名，按「用户数据根（__HOME__）下
 * webdav/legado/」的相对路径走 GET /reader3/file/download（file/download 的 path 是 home 根下相对路径）。
 */
export function downloadBackupZip(absPath: string): Promise<Blob> {
  const name = absPath.split(/[\\/]/).filter(Boolean).pop() || 'backup.zip'
  return downloadFile(`webdav/legado/${name}`, '__HOME__')
}
