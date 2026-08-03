import request from './request'
import type { ReturnData } from '@/types'

/**
 * POST /reader3/uploadLocalBook（multipart/form-data，字段名 file，单文件逐个上传）
 * 注意：FormData 交给 axios 自动设置 Content-Type（含 boundary），切勿手动指定。
 * 上传大文件放宽超时；onProgress 回传 0-100 百分比。
 */
export function uploadLocalBook(
  file: File,
  onProgress?: (percent: number) => void,
): Promise<ReturnData<unknown>> {
  const form = new FormData()
  form.append('file', file)
  return request
    .post('/uploadLocalBook', form, {
      timeout: 120_000,
      onUploadProgress: (e) => {
        if (onProgress && e.total) {
          onProgress(Math.round((e.loaded / e.total) * 100))
        }
      },
    })
    .then((r) => r.data as ReturnData<unknown>)
}
