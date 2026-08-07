/**
 * 全局确认对话框（ElMessageBox 兼容层渲染组件）。
 * 状态由 lib/confirm.ts 单例驱动，App.vue 挂载一次即可全局可用。
 */
<script setup lang="ts">
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { confirmState, settleConfirm } from '@/lib/confirm'
</script>

<template>
  <Dialog :open="confirmState.visible" @update:open="(v: boolean) => !v && settleConfirm(false)">
    <DialogContent class="sm:max-w-[425px]">
      <DialogHeader>
        <DialogTitle>{{ confirmState.title }}</DialogTitle>
      </DialogHeader>
      <DialogBody>
        <p class="text-sm text-muted-foreground whitespace-pre-line">{{ confirmState.message }}</p>
      </DialogBody>
      <DialogFooter>
        <Button
          v-if="confirmState.cancelText"
          variant="outline"
          @click="settleConfirm(false)"
        >
          {{ confirmState.cancelText }}
        </Button>
        <Button
          :variant="confirmState.type === 'error' ? 'destructive' : 'default'"
          @click="settleConfirm(true)"
        >
          {{ confirmState.confirmText }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
