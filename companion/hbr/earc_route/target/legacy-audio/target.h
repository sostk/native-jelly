/* SPDX-License-Identifier: GPL-2.0-only */
/* LG's old AUDIO_* driver on webOS 6 (kernel 4.4.84-229: C1, W21K and other 2021 models).
 * No address is built in: each function comes from kallsyms and must have the analysed
 * shape (core/code.h), and the data the hooks read is derived from LG's own code. */
#ifndef EARC_TARGET_LEGACY_AUDIO_H
#define EARC_TARGET_LEGACY_AUDIO_H

/* X(name, instructions, shape). The kernel's own break-hook and patching code first. */
#define EARC_FUNCTIONS(X) \
    X(brk_handler, 48, 0xf9b3ebae) \
    X(register_break_hook, 22, 0x1b85b958) \
    X(unregister_break_hook, 22, 0xa9f2f344) \
    X(aarch64_insn_patch_text, 48, 0x5b32365f) \
    X(__aarch64_insn_write, 68, 0x2b0c4fa9) \
    X(AUDIO_SetIecOutEncodedType, 76, 0x1807a087) \
    X(sndout_earc_output_type, 98, 0x2e09f863) \
    X(_AUDIO_SetSRCFormat, 64, 0x126d7054) \
    X(sndout_earc_onoff, 62, 0xfcee5c03) \
    X(AUDIO_GetMainAudioIndex, 22, 0x5e81df6b) \
    X(sndout_connect_output, 246, 0x5f1b9283) \
    X(_arc_connect_output, 224, 0x757c904c) \
    X(_AUDIO_ConnectRenderer, 190, 0xfbd15345) \
    X(_AUDIO_ConnectOutput_FindOutCtrlModule, 290, 0x6d51cac6) \
    X(_AUDIO_ConnectOutput_FindConnectModule, 508, 0x4d33bae8) \
    X(_AUDIO_ConnectOutput_FindOutModule, 352, 0x47d00ffc)
#include "../../core/code.h"

/* Encoded types: 3 = ARC port; 6 = MAT, LG's HBR passthrough. The shapes pin these values
 * and offsets: renderer + 0x1b0 = device and device + 0x8c = index (_AUDIO_ConnectRenderer),
 * request + 0x68 = mix flag, and _AUDIO_SetSRCFormat's format at sp + 0x40. */
static const struct earc_hook earc_hooks[] = {
    /* AUDIO_SetIecOutEncodedType(port, type): eARC carries the leased stream as HBR. */
    {EF_AUDIO_SetIecOutEncodedType, 0, 0xa9bd7bfd, WHEN_ANY_LEASE_PORT(0, 3), SET_REG(1, EARC_PARAM_VALUE)},
    /* sndout_earc_output_type(kcontrol, request): clear LG's eARC mix flag. */
    {EF_sndout_earc_output_type, 0, 0xd10203ff, WHEN_LEASE(), CLEAR_U64(1, 0x68)},
    /* _AUDIO_SetSRCFormat before its output defaults: x19 = renderer. */
    {EF__AUDIO_SetSRCFormat, 0x6c, 0x52800400, WHEN_LEASE_DEVICE(19, 0x1b0, 0x8c), CARRIER(0x40)},
    /* sndout_connect_output (LG's inlined _earc_connect_output) at `add x0, sp, #0x90`, before
     * it asks for the main audio: the carrier plays on the leased renderer (dtsdec's sink
     * index), not on the main decoder, which has no decoder info then (should_connect 0).
     * Take LG's bypass branch at its device lookup (+0x2c4: AUDIO_RM_GetDeviceIndex(x0, x1, x2)
     * from the conn_* parameters, then SetIecOutEncodedType(3, 5), connect eARC, arc_bypass) for
     * the leased renderer, with no decoder info (x22) and output 5 (w23). */
    {EF_sndout_connect_output, 0x94, 0x910243e0, WHEN_ANY_LEASE(), CONNECT_LEASED(23, 5, 0x90, 0x2c4)},
    /* _AUDIO_ConnectOutput_FindOutModule, eARC port, at `ldp w6, w7, [sp, #0x44]` just before it
     * sends the ARC module its es-out (sp + 0x44), mode (+0x48) and channels (+0x4c) and the eARC
     * channel type (x21 + 0xf68): override each from the es_* parameters (unset: LG's value). */
    /* _AUDIO_ConnectOutput_FindOutCtrlModule, ES-renderer branch, at `mov w28, w25`: the input rate
     * that becomes the eARC transmitter clock and format rate (LG's default 48000 without decoder
     * info). es_rate (e.g. 192000) replaces it; 0 leaves LG's value. */
    {EF__AUDIO_ConnectOutput_FindOutCtrlModule, 0x22c, 0x2a1903fc, WHEN_ANY_LEASE(), SET_REG(25, EARC_PARAM_RATE)},
    {EF__AUDIO_ConnectOutput_FindOutModule, 0xc4, 0x29489fe6, WHEN_ANY_LEASE(), ES_FORCE(0x44, 21, 0xf68)},
};

/* LG's eARC-on byte and main renderer index: the one byte store in sndout_earc_onoff and the
 * one word load in AUDIO_GetMainAudioIndex, cross-checked against their kallsyms names. */
static volatile u8 *earc_on;
static volatile u32 *earc_main_index;
/* LG's eARC output mode/format (0 PCM, 1 ES), read by KDRV_AUDIO_UpdateArcSamplingFreq for the
 * transmitter format; only the es_out experiment writes it. */
static volatile u32 *earc_arc_format;

static const char *earc_target_derive(const struct earc_code *fns)
{
    struct earc_scan scan;
    earc_scan(&fns[EF_sndout_earc_onoff], 1, 1, &scan);
    if (scan.found != 1 || scan.address != kallsyms_lookup_name("_gSndOutEarcOnOff"))
        return "_gSndOutEarcOnOff";
    earc_on = (volatile u8 *)scan.address;
    earc_scan(&fns[EF_AUDIO_GetMainAudioIndex], 4, 0, &scan);
    if (scan.found != 1 || scan.address != kallsyms_lookup_name("_gMainAudioIndex"))
        return "_gMainAudioIndex";
    earc_main_index = (volatile u32 *)scan.address;
    earc_arc_format = (volatile u32 *)kallsyms_lookup_name("gArcOutModeFormat");
    return 0;
}

#endif
