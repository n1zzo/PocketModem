-- WirePlumber: Beamforming filter for DualMic
-- Automatically links DualMic source through a LADSPA delay-and-sum filter

local Log = require("log")
local core = require("core")
local deferred = require("deferred")

local log = Log.new_info("beamform")

log:info("Beamforming: Loading WirePlumber config")

-- Configuration
local DELAY_MS = 0.44  -- Delay in ms for ~15cm mic spacing
local GAIN = 0.5       -- Gain per channel to prevent clipping

-- Find the beamforming filter node
local filter_node = nil

local function createBeamformFilter()
    log:info("Beamforming: Creating filter node")
    
    -- Create a filter chain using the adapter module
    local props = {
        ["node.name"] = "beamform-filter",
        ["node.description"] = "Beamforming Filter",
        ["factory.name"] = "support.node",
        ["media.class"] = "Filter/Audio/Source",
    }
    
    -- Load the filter chain module
    local result = core:load_module("libpipewire-module-filter-chain", {
        ["filter.match"] = {
            ["node.name"] = "alsa_input.platform-sound.*DualMic*"
        },
        ["filter.factory"] = {
            ["name"] = "filter-chain"
        }
    })
    
    if result then
        log:info("Beamforming: Filter chain module loaded")
        return true
    else
        log:warning("Beamforming: Failed to load filter chain module")
        return false
    end
end

-- Monitor for new DualMic sources
local function onNodeCreated(node)
    local name = node.properties["node.name"]
    if name and string.match(name, ".*DualMic.*") then
        log:info("Beamforming: Detected DualMic source: " .. name)
        
        -- The filter should auto-apply based on filter-chain.conf.d
        -- This is a placeholder for any custom processing needed
    end
end

-- Register for node events
core:connect("node-created", onNodeCreated)

log:info("Beamforming: Config loaded, waiting for DualMic sources")