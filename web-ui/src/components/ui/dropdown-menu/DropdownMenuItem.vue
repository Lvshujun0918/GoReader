<script setup lang="ts">
import type { HTMLAttributes } from 'vue'
import { DropdownMenuItem as Primitive } from 'reka-ui'
import { cn } from '@/lib/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()
</script>

<template>
  <Primitive
    :class="
      cn(
        'relative flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none transition-colors focus:bg-accent focus:text-accent-foreground data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground',
        props.class,
      )
    "
  >
    <slot />
  </Primitive>
</template>
