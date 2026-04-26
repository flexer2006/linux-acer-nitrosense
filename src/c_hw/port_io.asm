; SPDX-License-Identifier: GPL-3.0-or-later
; Copyright (C) 2024-2026 NitroSense Contributors

format ELF64

; ---------------------------------------------------------------------------
; NitroSense x86_64 port I/O and MSR primitives
; Calling convention: System V AMD64 ABI
;   args: rdi, rsi, rdx, rcx, r8, r9
;   return: rax
;   caller-saved: rax, rcx, rdx, rsi, rdi, r8-r11
; ---------------------------------------------------------------------------

public asm_inb
public asm_outb
public asm_rdmsr
public asm_wrmsr

; ---------------------------------------------------------------------------
; uint8_t asm_inb(uint16_t port)
;   input:  di = port number
;   output: al = byte read from port (zero-extended to eax)
;   clobbers: none beyond rax, rdx
; ---------------------------------------------------------------------------
asm_inb:
    movzx edx, di           ; port -> dx
    xor   eax, eax          ; clear rax
    in    al, dx             ; read byte from port
    ret

; ---------------------------------------------------------------------------
; void asm_outb(uint16_t port, uint8_t val)
;   input:  di = port number, sil = value
;   output: none
;   clobbers: rax, rdx
; ---------------------------------------------------------------------------
asm_outb:
    movzx edx, di           ; port -> dx
    mov   al, sil            ; value -> al
    out   dx, al             ; write byte to port
    ret

; ---------------------------------------------------------------------------
; uint64_t asm_rdmsr(uint32_t msr)
;   input:  edi = MSR index
;   output: rax = 64-bit MSR value (edx:eax combined)
;   clobbers: rcx, rdx
; ---------------------------------------------------------------------------
asm_rdmsr:
    mov   ecx, edi           ; MSR index -> ecx
    rdmsr                    ; result in edx:eax
    shl   rdx, 32            ; shift high 32 bits into position
    or    rax, rdx            ; combine into rax
    ret

; ---------------------------------------------------------------------------
; void asm_wrmsr(uint32_t msr, uint64_t val)
;   input:  edi = MSR index, rsi = 64-bit value
;   output: none
;   clobbers: rax, rcx, rdx
; ---------------------------------------------------------------------------
asm_wrmsr:
    mov   ecx, edi           ; MSR index -> ecx
    mov   eax, esi           ; low 32 bits of val -> eax
    mov   rdx, rsi
    shr   rdx, 32            ; high 32 bits of val -> edx
    wrmsr                    ; write MSR
    ret
