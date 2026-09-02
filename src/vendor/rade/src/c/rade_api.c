/*---------------------------------------------------------------------------*

  rade_api.c

  Simplified C-only RADE API for Rust FFI bindings.
  This version uses the built-in C encoder/decoder without Python.

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

#include <assert.h>
#include <stdlib.h>
#include <string.h>

#include "rade_api.h"
#include "rade_core.h"
#include "rade_enc.h"
#include "rade_enc_data.h"
#include "rade_dec.h"
#include "rade_dec_data.h"
#include "rade_constants.h"
#include "os_support.h"

// ============================================================================
// Constants
// ============================================================================

#define RADE_API_VERSION 1

// ============================================================================
// RADE Context Structure
// ============================================================================

struct rade {
    int flags;
    int num_features;
    
    // TX state
    RADEEnc      enc_model;
    RADEEncState enc_state;
    float        features_buf[RADE_FEATURES_PER_STEP];
    float        latents[RADE_LATENT_DIM];
    
    // RX state  
    RADEDec      dec_model;
    RADEDecState dec_state;
    float        rx_features[RADE_FEATURES_PER_STEP];
    float        rx_latents[RADE_LATENT_DIM];
    
    // TX output buffers (OFDM modem)
    // These would normally be handled by the full rade_api.c with Python
    // For this simplified version, we just return the latent vector
    RADE_COMP    tx_buf[RADE_TX_FRAME_SIZE];
    RADE_COMP    tx_eoo_buf[RADE_TX_EOO_SIZE];
    
    // RX input buffer
    RADE_COMP    rx_buf[RADE_NIN_MAX];
};

// ============================================================================
// API Functions
// ============================================================================

void rade_initialize(void) {
    // No-op for C-only version
}

void rade_finalize(void) {
    // No-op for C-only version
}

struct rade *rade_open(char model_file[], int flags) {
    (void)model_file;  // Not used in C-only version
    
    struct rade *r = (struct rade *)calloc(1, sizeof(struct rade));
    if (r == NULL) return NULL;
    
    r->flags = flags;
    
    // Default feature count (20 LPCNet features)
    r->num_features = RADE_FEATURES_PER_FRAME;
    if (flags & 0x8) {
        r->num_features += 1;  // Auxiliary data
    }
    
    int input_dim = r->num_features * RADE_FRAMES_PER_STEP;
    
    // Initialize encoder
    if (flags & RADE_USE_C_ENCODER) {
        if (init_radeenc(&r->enc_model, radeenc_arrays, input_dim) != 0) {
            free(r);
            return NULL;
        }
        rade_init_encoder(&r->enc_state);
    }
    
    // Initialize decoder
    if (flags & RADE_USE_C_DECODER) {
        if (init_radedec(&r->dec_model, radedec_arrays, input_dim) != 0) {
            free(r);
            return NULL;
        }
        rade_init_decoder(&r->dec_state);
    }
    
    return r;
}

void rade_close(struct rade *r) {
    if (r != NULL) {
        free(r);
    }
}

int rade_version(void) {
    return RADE_API_VERSION;
}

int rade_n_tx_out(struct rade *r) {
    (void)r;
    return RADE_TX_FRAME_SIZE;
}

int rade_n_tx_eoo_out(struct rade *r) {
    (void)r;
    return RADE_TX_EOO_SIZE;
}

int rade_nin_max(struct rade *r) {
    (void)r;
    return RADE_NIN_MAX;
}

int rade_n_features_in_out(struct rade *r) {
    return r->num_features * RADE_FRAMES_PER_STEP;
}

int rade_tx(struct rade *r, RADE_COMP tx_out[], float features_in[]) {
    if (!(r->flags & RADE_USE_C_ENCODER)) {
        return 0;  // C encoder not enabled
    }
    
    int arch = 0;  // Generic arch (could be optimized)
    int bottleneck = 1;  // Use bottleneck (tanh activation)
    
    // Encode features to latent vector
    rade_core_encoder(&r->enc_state, &r->enc_model, r->latents, features_in, arch, bottleneck);
    
    // For now, just copy latent vector to output (caller should do OFDM modulation)
    // In the full implementation, this would call Python's radae_txe
    for (int i = 0; i < RADE_LATENT_DIM && i < RADE_TX_FRAME_SIZE; i++) {
        tx_out[i].real = r->latents[i];
        tx_out[i].imag = 0.0f;
    }
    
    return RADE_LATENT_DIM;
}

int rade_tx_eoo(struct rade *r, RADE_COMP tx_eoo_out[]) {
    if (!(r->flags & RADE_USE_C_ENCODER)) {
        return 0;
    }
    
    // End of over - copy any remaining data
    for (int i = 0; i < RADE_TX_EOO_SIZE; i++) {
        tx_eoo_out[i].real = 0.0f;
        tx_eoo_out[i].imag = 0.0f;
    }
    
    return RADE_TX_EOO_SIZE;
}

int rade_nin(struct rade *r) {
    (void)r;
    // Return number of input samples (complex) for one decode step
    // When using C decoder, this is the latent dimension
    // In full system with OFDM, this would be the OFDM frame size
    if (r->flags & RADE_USE_C_DECODER) {
        return RADE_LATENT_DIM;
    }
    return RADE_NIN_MAX;  // Full OFDM frame size
}

int rade_rx(struct rade *r, float features_out[], RADE_COMP rx_in[]) {
    if (!(r->flags & RADE_USE_C_DECODER)) {
        return 0;  // C decoder not enabled
    }
    
    int arch = 0;  // Generic arch
    
    // Extract latent vector from rx_in (caller should do OFDM demodulation first)
    // For now, just copy from rx_in assuming it's the latent vector
    for (int i = 0; i < RADE_LATENT_DIM && i < RADE_NIN_MAX; i++) {
        r->rx_latents[i] = rx_in[i].real;  // Use real part as latent value
    }
    
    // Decode latent vector to features
    rade_core_decoder(&r->dec_state, &r->dec_model, r->rx_features, r->rx_latents, arch);
    
    // Copy output features
    for (int i = 0; i < RADE_FEATURES_PER_STEP; i++) {
        features_out[i] = r->rx_features[i];
    }
    
    return RADE_FEATURES_PER_STEP;
}

int rade_sync(struct rade *r) {
    (void)r;
    // Simplified: always in sync if decoder is enabled
    return (r->flags & RADE_USE_C_DECODER) ? 1 : 0;
}

float rade_freq_offset(struct rade *r) {
    (void)r;
    return 0.0f;  // No frequency offset estimation in simplified version
}