<script setup lang="ts">
import { computed, useAttrs } from 'vue'

export type ButtonVariant =
  | 'primary'
  | 'secondary'
  | 'ghost'
  | 'danger'
  | 'dangerQuiet'
export type ButtonSize = 'sm' | 'md' | 'icon' | 'iconMd' | 'iconSm'

/**
 * Only `primary` spends the brand gradient, and it keeps forest ink rather
 * than white so the lime sweep stays legible in both themes.
 *
 * Inactive primary uses grayscale + desaturation rather than a faint opacity
 * fade — opacity alone left the gradient looking merely washed out.
 */
const VARIANT_CLASSES = {
  primary:
    'bg-brand-gradient text-cta-ink shadow-panel hover:brightness-105 active:brightness-95 disabled:grayscale disabled:saturate-0 disabled:brightness-75 aria-disabled:grayscale aria-disabled:saturate-0 aria-disabled:brightness-75',
  secondary:
    'border border-line-strong bg-surface text-ink hover:border-accent/40 hover:bg-accent/8',
  ghost: 'text-ink-muted hover:bg-accent/10 hover:text-ink',
  danger: 'border border-critical/35 text-critical hover:bg-critical/10',
  dangerQuiet:
    'border border-line-strong text-ink-muted hover:border-critical/35 hover:bg-critical/10 hover:text-critical',
} satisfies Record<ButtonVariant, string>

const SIZE_CLASSES = {
  sm: 'h-8 gap-1.5 px-3 text-xs',
  md: 'h-10 gap-2 px-4 text-sm',
  /** Matches `sm` height for toolbar icon controls beside text buttons. */
  iconSm: 'size-8 p-0',
  /** Comfortable target for a control that sits inside a panel it must not
      dominate, such as dismissing a notice. */
  iconMd: 'size-9 p-0',
  /** Large touch target for app chrome (open/close panels). */
  icon: 'size-11 p-0',
} satisfies Record<ButtonSize, string>

defineOptions({ inheritAttrs: false })

const {
  variant = 'secondary',
  size = 'md',
  type = 'button',
} = defineProps<{
  variant?: ButtonVariant
  size?: ButtonSize
  type?: 'button' | 'submit'
}>()

const attrs = useAttrs()

const forwardedAttrs = computed(() => {
  const rest = { ...(attrs as Record<string, unknown>) }
  delete rest.onClick
  return rest
})

function isAriaDisabled(): boolean {
  const value = attrs['aria-disabled']
  return value === true || value === '' || value === 'true'
}

function onClick(event: MouseEvent): void {
  // Honour aria-disabled the same way as native disabled: block activation.
  if (isAriaDisabled()) {
    event.preventDefault()
    event.stopImmediatePropagation()
    return
  }

  const handler = attrs.onClick
  if (typeof handler === 'function') {
    handler(event)
    return
  }
  if (Array.isArray(handler)) {
    for (const listener of handler) {
      if (typeof listener === 'function') listener(event)
    }
  }
}
</script>

<template>
  <button
    v-bind="forwardedAttrs"
    :type="type"
    class="inline-flex shrink-0 items-center justify-center rounded-control font-medium transition-[background-color,border-color,color,filter,opacity] duration-150 disabled:cursor-not-allowed disabled:opacity-55 aria-disabled:cursor-not-allowed aria-disabled:opacity-55"
    :class="[VARIANT_CLASSES[variant], SIZE_CLASSES[size]]"
    @click="onClick"
  >
    <slot />
  </button>
</template>
