import request from './request'
import type { ReturnData, FileItem } from '@/types'

/**
 * GET /reader3/file/list：文件列表
 * @param path 当前目录（根目录传空串）
 * @param home 可选：__LOCAL_STORE__=书仓 / __HOME__=用户数据 / __WEBDAV__=WebDAV / 空=用户根
 */
export function listFiles(path: string, home = ''): Promise<ReturnData<FileItem[]>> {
  return request
    .get('/file/list', { params: { path, ...(home ? { home } : {}) } })
    .then((r) => r.data as ReturnData<FileItem[]>)
}

/** GET /reader3/file/get：读取文本文件内容 */
export function getFile(path: string): Promise<ReturnData<string>> {
  return request.get('/file/get', { params: { path } }).then((r) => r.data as ReturnData<string>)
}

/** POST /reader3/file/save：写入文本文件（body { path, content }） */
export function saveFile(path: string, content: string): Promise<ReturnData<null>> {
  return request.post('/file/save', { path, content }).then((r) => r.data as ReturnData<null>)
}

/** POST /reader3/file/mkdir：新建文件夹（body { path }） */
export function mkdir(path: string): Promise<ReturnData<null>> {
  return request.post('/file/mkdir', { path }).then((r) => r.data as ReturnData<null>)
}

/** GET /reader3/file/download：下载文件，返回 Blob（大文件放宽超时） */
export function downloadFile(path: string): Promise<Blob> {
  return request
    .get('/file/download', { params: { path }, responseType: 'blob', timeout: 120_000 })
    .then((r) => r.data as Blob)
}

/** POST /reader3/file/upload：multipart 上传（字段 file + path，FormData 交 axios 设 Content-Type） */
export function uploadFile(file: File, path: string): Promise<ReturnData<null>> {
  const form = new FormData()
  form.append('file', file)
  form.append('path', path)
  return request
    .post('/file/upload', form, { timeout: 120_000 })
    .then((r) => r.data as ReturnData<null>)
}

/** POST /reader3/file/delete：删除文件/目录（body { path }） */
export function deleteFile(path: string): Promise<ReturnData<null>> {
  return request.post('/file/delete', { path }).then((r) => r.data as ReturnData<null>)
}
