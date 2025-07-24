- [ ] Fallback to CPU for all image processors
- [ ] If compositing > 7 images then just batch into 7 image batches and run each and then composite them together (supports up to 49 images which should be enough for pretty much everything)
- [ ] Abstract drop menu in tools/manager.rs into GUI component
- [ ] Colors are inverted when loading a single image
- [ ] Latency from clicking to count showing up is enormoues
- [ ] Auto counter not getting new state for `Count {x}` text???
- [X] Proper pipeline tool caching (cache output and then invalidate downstream caches in Refresh command handler)
- [ ] Run Lutz on GPU
- [ ] Instead of using PipelineCommand for all channels break it up so only nessecary commands need to be handled in match

## Features:
- [ ] Support dragging and dropping images into viewer
- [ ] Support Z-stacking
- [ ] Support stitching
- [X] Thresholding
- [ ] Watershed segmentation
- [X] Local maxima detection/segmentation
- [ ] Weka segmentation with weka-rs
- [X] Manual/Autocell counter
- [ ] Background subtraction
- [ ] Flourescene intensity measurment tool
- [ ] Colocalization analysis tool
- [ ] Saving and loading pipelines
- [ ] Morphological operations (Binary dilation, erosion, opening, closing)
- [ ] Allow user to type values directly into value control
- [ ] Add mode toggle to "Threshold" that in addition to manual thresholding supports otsu thresholding
- [ ] Settings menu with option to force CPU processing
- [ ] Multiple tool instances
- [ ] Grayscale tool
- [ ] Make it so swithcing between auto and manual does not overwrite manual counts
- [ ] Slicer/Splitter thingy that takes in one image and splits out each color channel as it's own image

## Big features:
- [ ] Parallel node based editor instead of linear flow
- [ ] Scripting with Rhai: https://github.com/rhaiscript/rhai
- [ ] Plugins
