import request from './request'

/**
 * 书籍导出 —— 后端契约（mobi/azw3 后端开发中，未就绪时返回错误 JSON 由调用方降级提示）
 *
 * GET /reader3/exportBook?url=<bookUrl>&format=txt|epub|html|mobi|azw3&encoding=utf-8|gbk
 *   → 成功：对应格式附件（blob 下载）
 *   → 失败：legacy ReturnData JSON 错误体（由调用方经 utils/download.ts downloadBlob 识别提示）
 * encoding 参数仅 txt 生效（后端并行实现中——未就绪时忽略，仍输出 UTF-8）。
 */

export type ExportFormat = 'txt' | 'epub' | 'html' | 'mobi' | 'azw3'

export type ExportEncoding = 'utf-8' | 'gbk'

/** GET /reader3/exportBook：导出本书为指定格式（blob，文件名由调用方拼 bookName.format） */
export function exportBook(
  url: string,
  format: ExportFormat,
  encoding: ExportEncoding = 'utf-8',
): Promise<Blob> {
  const params: Record<string, string> = { url, format }
  if (format === 'txt') params.encoding = encoding
  return request
    .get('/exportBook', {
      params,
      responseType: 'blob',
      timeout: 120_000,
      silent: true,
    })
    .then((r) => r.data as Blob)
}
