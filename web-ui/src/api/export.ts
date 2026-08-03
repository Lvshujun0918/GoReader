import request from './request'

/**
 * 书籍导出 —— 后端契约（并行实现中，未就绪时 404/网络失败由调用方 silent 降级提示）
 *
 * GET /reader3/exportBook?url=<bookUrl>&format=txt|epub|html
 *   → 成功：对应格式附件（blob 下载）
 *   → 失败：legacy ReturnData JSON 错误体（由调用方经 utils/download.ts downloadBlob 识别提示）
 */

export type ExportFormat = 'txt' | 'epub' | 'html'

/** GET /reader3/exportBook：导出本书为指定格式（blob，文件名由调用方拼 bookName.format） */
export function exportBook(url: string, format: ExportFormat): Promise<Blob> {
  return request
    .get('/exportBook', {
      params: { url, format },
      responseType: 'blob',
      timeout: 120_000,
      silent: true,
    })
    .then((r) => r.data as Blob)
}
