<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import {
  DropdownMenuContent as Primitive,
  DropdownMenuPortal,
} from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
  align?: 'start' | 'center' | 'end'
}>()
</script>

<template>
  <DropdownMenuPortal>
    <Primitive
      :align="align ?? 'end'"
      :class="
        cn(
          'z-50 min-w-[9rem] overflow-hidden rounded-md border bg-background p-1 text-foreground shadow-md data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95',
          props.class,
        )
      "
    >
      <slot />
    </Primitive>
  </DropdownMenuPortal>
</template>
