- [ ] Fallback to CPU for all image processors
- [ ] If compositing > 7 images then just batch into 7 image batches and run each and then composite them together (supports up to 49 images which should be enough for pretty much everything)
- [ ] Abstract drop menu in tools/manager.rs into GUI component
- [X] Colors are inverted when loading a single image
- [X] Proper pipeline tool caching (cache output and then invalidate downstream caches in Refresh command handler)
- [ ] Run Lutz on GPU
- [ ] Instead of using PipelineCommand for all channels break it up so only nessecary commands need to be handled in match
- [ ] Cleanup BackendTool trait so tool manager does not need to have a bunch of stuff it doesn't need
- [ ] If tokio backend panics handle it properly
- [ ] GUI Error handling
- [ ] Debounce pipeline events so if the user is changing the state rapdily like with a value counter it is all processed in a bundle
- [ ] Add debouncing to frontend value counter
- [ ] Make it so compositor is just a normal tool that can be disabled and enabled just like any other and then at the end of the pipeline just check if images.len() > 1 and if so just composite them together and send that to the output. Compositor should still be enabled by default when loading multiple images.

## Features:
- [ ] Support dragging and dropping images into viewer
- [ ] Support Z-stacking
- [ ] Support stitching
- [ ] Themes
- [X] Thresholding
- [X] Watershed segmentation
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


## Known issues:

This does not happen in release mode, nothing bad seems to happen so it is probably fine... : Crashes when trying to load CH2_weka_prepped.tif: assertion failed: self.table.is_full() ||
        (self.code_buffer.code_size() == 2 && self.next_code == 4) ||
    self.code_buffer.max_code() - Code::from(self.is_tiff) >= self.next_code
