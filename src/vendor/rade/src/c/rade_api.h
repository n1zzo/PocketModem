/*---------------------------------------------------------------------------*

  rade_api.h

  Simplified C-only RADE API for Rust FFI bindings.

\*---------------------------------------------------------------------------*/

/*
  Copyright (C) 2024 David Rowe, Jan Buethe

  Redistribution and use in source and binary forms, with or without
  modification, are permitted provided that the following conditions
  are met:

  - Redistributions of source code must retain the above copyright
  notice, this list of conditions and the following disclaimer.

  - Redistributions in binary form must reproduce the above copyright
  notice, this list of conditions and the following disclaimer in the
  documentation and/or other materials provided with the distribution.

  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
  ``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
  LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
  A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION OR
  CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
  EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
  PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
  PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
  LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
  NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
  SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

*/

#ifndef __RADE_API__
#define __RADE_API__

#include <sys/types.h>

// This declares a single-precision (float) complex number
typedef struct {
  float real;
  float imag;
} RADE_COMP;

// Sample rates used
#define RADE_MODEM_SAMPLE_RATE 8000           // modem waveform sample rate
#define RADE_SPEECH_SAMPLE_RATE 16000         // speech sample rate

// init rade_open() flags
#define RADE_USE_C_ENCODER 0x1
#define RADE_USE_C_DECODER 0x2
#define RADE_FOFF_TEST     0x4                // test mode used only by developers
#define RADE_VERBOSE_0     0x8                // reduce verbosity to "quiet"

// Frame dimensions
#define RADE_LATENT_DIM       80              // latent vector size
#define RADE_FRAMES_PER_STEP  4               // frames per encode/decode step
#define RADE_FEATURES_PER_FRAME 20            // LPCNet features per frame

// Derived constants
#define RADE_FEATURES_PER_STEP (RADE_FRAMES_PER_STEP * RADE_FEATURES_PER_FRAME)
#define RADE_TX_FRAME_SIZE     512            // complex samples per TX frame
#define RADE_TX_EOO_SIZE       256            // complex samples for end-of-over
#define RADE_NIN_MAX           3200           // max RX input samples

// Must be called BEFORE any other RADE functions as this
// initializes internal library state.
void rade_initialize(void);

// Should be called when done with RADE.
void rade_finalize(void);

// Open RADE context
struct rade *rade_open(char model_file[], int flags);
void rade_close(struct rade *r);

// Returns API version
int rade_version(void);

// TX functions
int rade_n_tx_out(struct rade *r);
int rade_n_tx_eoo_out(struct rade *r);
int rade_tx(struct rade *r, RADE_COMP tx_out[], float features_in[]);
int rade_tx_eoo(struct rade *r, RADE_COMP tx_eoo_out[]);

// RX functions
int rade_nin_max(struct rade *r);
int rade_nin(struct rade *r);
int rade_n_features_in_out(struct rade *r);
int rade_rx(struct rade *r, float features_out[], RADE_COMP rx_in[]);
int rade_sync(struct rade *r);
float rade_freq_offset(struct rade *r);

#endif  //__RADE_API__