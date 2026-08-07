/**
 * Element Plus 兼容层 → shadcn-vue（sonner toast + 全局确认框）。
 *
 * 前端从 Element Plus 迁移到 shadcn-vue 的过渡层：
 * 保留 `ElMessage` / `ElMessageBox` 的 API 形状（全项目 175 处调用无需改动），
 * 底层替换为 shadcn 风格的 sonner toast 与 reka-ui Dialog。
 * 新代码请直接使用 sonner 的 `toast` 与 UI 组件。
 */
import { toast } from 'vue-sonner'
import { showConfirm } from '@/lib/confirm'

interface MessageBoxOptions {
  confirmButtonText?: string
  cancelButtonText?: string
  type?: 'success' | 'warning' | 'info' | 'error'
  /** Element Plus 兼容：区分取消与关闭（简化实现：一律视为取消） */
  distinguishCancelAndClose?: boolean
  inputValue?: string
  /** Element Plus 兼容：输入校验（简化实现：不校验） */
  inputPattern?: RegExp
  inputErrorMessage?: string
}

export const ElMessage = {
  success(msg: unknown) {
    toast.success(String(msg))
  },
  error(msg: unknown) {
    toast.error(String(msg))
  },
  warning(msg: unknown) {
    toast.warning(String(msg))
  },
  info(msg: unknown) {
    toast.info(String(msg))
  },
  message(msg: unknown) {
    toast(String(msg))
  },
  /** 关闭当前 toast（sonner 兼容：dismiss 最新） */
  closeAll() {
    toast.dismiss()
  },
}

/** ElMessageBox 兼容：confirm/alert → Promise<boolean>；prompt → Promise<{value, action}> */
export const ElMessageBox = {
  /** 确认框：确认 → true，取消 → false */
  async confirm(message: string, title?: string, opts?: MessageBoxOptions): Promise<boolean> {
    return showConfirm({
      title: title || '提示',
      message,
      confirmText: opts?.confirmButtonText ?? '确定',
      cancelText: opts?.cancelButtonText ?? '取消',
      type: opts?.type === 'error' ? 'error' : opts?.type === 'warning' ? 'warning' : 'info',
    })
  },

  /** 提示框：确认 → true，无取消 */
  async alert(message: string, title?: string, opts?: MessageBoxOptions): Promise<boolean> {
    return showConfirm({
      title: title || '提示',
      message,
      confirmText: opts?.confirmButtonText ?? '确定',
      cancelText: '',
      type: 'info',
    })
  },

  /** 输入框（Element Plus 形状：resolve { value, action }） */
  async prompt(
    message: string,
    title?: string,
    opts?: MessageBoxOptions,
  ): Promise<{ value: string; action: 'confirm' | 'cancel' }> {
    const ok = await showConfirm({
      title: title || '提示',
      message,
      confirmText: opts?.confirmButtonText ?? '确定',
      cancelText: opts?.cancelButtonText ?? '取消',
      type: 'info',
    })
    if (ok) return { value: opts?.inputValue ?? '', action: 'confirm' }
    return { value: '', action: 'cancel' }
  },
}

/** 兼容 `import ElementPlus from 'element-plus'` 形态（默认导出） */
export default { ElMessage, ElMessageBox }
