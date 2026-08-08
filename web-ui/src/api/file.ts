import request, { type RequestOptions } from './request'
import type { ReturnData } from '@/types'

/**
 * POST /reader3/file/mkdir：创建目录（书封/背景图上传前确保目录存在）。
 * @param parent 父目录（根目录传空串）
 * @param name 目录名
 */
export function mkdir(
  parent: string,
  name: string,
  home = '',
  opts?: RequestOptions,
): Promise<ReturnData<null>> {
  return request
    .post('/file/mkdir', { path: parent, name, ...(home ? { home } : {}) }, { silent: opts?.silent })
    .then((r) => r.data as ReturnData<null>)
}

/** GET /reader3/file/download：下载文件，返回 Blob（大文件放宽超时；备份 zip 下载用） */
export function downloadFile(path: string, home = ''): Promise<Blob> {
  return request
    .get('/file/download', {
      params: { path, ...(home ? { home } : {}) },
      responseType: 'blob',
      timeout: 120_000,
    })
    .then((r) => r.data as Blob)
}

/**
 * POST /reader3/file/upload：multipart 上传（字段 file + path，FormData 交 axios 设 Content-Type）
 * @param onProgress 上传进度回调（0-100）
 */
export function uploadFile(
  file: File,
  path: string,
  home = '',
  onProgress?: (percent: number) => void,
): Promise<ReturnData<null>> {
  const form = new FormData()
  form.append('file', file)
  form.append('path', path)
  if (home) form.append('home', home)
  return request
    .post('/file/upload', form, {
      timeout: 120_000,
      onUploadProgress: onProgress
        ? (e) => {
            if (e.total) onProgress(Math.min(100, Math.round((e.loaded / e.total) * 100)))
          }
        : undefined,
    })
    .then((r) => r.data as ReturnData<null>)
}
