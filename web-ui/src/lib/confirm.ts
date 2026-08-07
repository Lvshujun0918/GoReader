/**
 * 全局确认对话框单例状态（ElMessageBox 兼容层）。
 * 由 GlobalConfirmDialog.vue 渲染，feedback.ts 的 confirm 流程驱动。
 */
import { reactive } from 'vue'

export interface ConfirmState {
  visible: boolean
  title: string
  message: string
  confirmText: string
  cancelText: string
  type: 'info' | 'warning' | 'error'
  resolve: ((ok: boolean) => void) | null
}

export const confirmState = reactive<ConfirmState>({
  visible: false,
  title: '提示',
  message: '',
  confirmText: '确定',
  cancelText: '取消',
  type: 'info',
  resolve: null,
})

/** 打开确认框，返回 Promise<boolean>（true=确认，false=取消） */
export function showConfirm(opts: {
  title?: string
  message: string
  confirmText?: string
  cancelText?: string
  type?: ConfirmState['type']
}): Promise<boolean> {
  return new Promise((resolve) => {
    confirmState.visible = true
    confirmState.title = opts.title ?? '提示'
    confirmState.message = opts.message
    confirmState.confirmText = opts.confirmText ?? '确定'
    confirmState.cancelText = opts.cancelText ?? '取消'
    confirmState.type = opts.type ?? 'info'
    confirmState.resolve = resolve
  })
}

/** 结算并关闭 */
export function settleConfirm(result: boolean) {
  confirmState.visible = false
  confirmState.resolve?.(result)
  confirmState.resolve = null
}
