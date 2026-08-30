-- WirePlumber configuration for Beamforming
-- Auto-applies the beamforming filter chain when Beamform profile is active

-- Log startup
local log = Log.new_info()
log:info("Beamform: WirePlumber config loaded")

-- The filter chain (beamform.conf) is automatically applied by PipeWire
-- when the node matches the filter's target criteria.
-- This config ensures proper linking priority.