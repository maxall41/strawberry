## Alpha 0.1:
- [X] Decomposer tool that takes in one image and splits out each color channel as it's own image
- [X] Allow user to type values directly into value control + psuedo-slider
- [X] Saving and loading pipelines
- [X] CLI (Indiviudal, and directory processing supports regex for file filtering)
- [X] Support dragging and dropping images into viewer
- [X] Background subtraction with rolling ball
- [X] GUI Error handling with toasts
- [X] Sobel filter
- [X] Grayscale
- [X] Image export
- [ ] Fallback to CPU for all image processors with OpenCV: https://crates.io/crates/opencv
- [ ] If compositing > 7 images then just batch into 7 image batches and run each and then composite them together (supports up to 49 images which should be enough for pretty much everything)
- [X] Safe indexing
- [ ] Add delete currently selected count feature to counter review mode
- [ ] Keyboard shortcuts for EVERYTHING (See reMarkable)
- [ ] Move viewport to center of image on image load unless dimensions of new images match the current image dimensions.
- [X] Default counter to no tool selected
- [ ] Allow user to toggle display of each count type in Counter
- [X] Make sure CMD+Q quit app shortcut works

- [X] If tokio backend panics handle it properly
- [X] Refuses to load weka probability map file (the one that is 100+MB) with error: Failed to process pipeline after loading images: No input images provided
- [X] Make it so compositor is just a normal tool that can be disabled and enabled just like any other tool.
- [X] Debounce pipeline events so if the user is changing the state rapdily like with a value counter it is all processed in a bundle
- [X] Throw error when trying to composite images of different sizes
- [X] Don't allow user to enable compositor if there is only one image to prevent RGBA-BRGA mixups
- [ ] Fix Clippy Warnings
- [ ] Reset counter on new image load: Loading a new image for the second time and then loading the second set of counts causes a panic.
- [X] Compositor layers count does not update on new image load
- [ ] Turning a layer or adjusting contrast (or anything that pushes new image to viewer) on and off snaps the viewport back to the current review index.
- [ ] Build actual macos .app


## Alpha 0.2
- [ ] Command pallete
- [ ] Instead of using PipelineCommand for all channels break it up so only nessecary commands need to be handled in match
- [ ] Cleanup BackendTool trait so tool manager does not need to have a bunch of stuff it doesn't need
- [ ] Add mode toggle to "Threshold" that in addition to manual thresholding it supports Otsu thresholding, Local thresholding, and maybe one or two other thresholding methods.
- [ ] Kuwahara filter, Bilateral filter, and Median filter. Noise reduction
- [ ] If toast is error make time bar red and if sucessfull make it green & add icons to toasts
- [ ] Error toasts for frontend errors
- [ ] Replace human-panic with proper automatic panic reporting (with user prompt OK) & GUI
- [ ] Auto updater
- [ ] Documentation
- [ ] Welcome screen & onboarding
- [ ] Themes & Nicer GUI

# Alpha 0.3
- [ ] Flat field correction
- [ ] Deconvolution with Richardson-Lucy
- [ ] Cropping + General Selection tools (Line drawing, Rectangle drawing, Ellipse drawing, Lasso drawing)
- [ ] Flourescene intensity measurment tool (Use general selection tools)
- [ ] Submit feedback/feature request/bug report button
- [ ] Abstract drop menu in tools/manager.rs into GUI component & Use proper GPUI drag system to improve UX
- [ ] Difference of Guassians
- [ ] Run Lutz on GPU
- [ ] Support Z-stacking
- [ ] Support stitching
- [ ] Settings menu with option to force CPU processing + themes
- [ ] Multiple tool instances
- [ ] Undo & Redo
- [ ] Linux & Windows builds

## Other Features

- [ ] Try replacing Flume with Kanal and see if it improves performance
- [ ] Weka segmentation with weka-rs
- [ ] Colocalization analysis tool
- [ ] Non local means denoise

## Big features:
- [ ] Parallel node based editor instead of linear flow
- [ ] Scripting with RustPython: https://crates.io/crates/rustpython
- [ ] Plugins
- [ ] Backend on a server

## Known issues:

This does not happen in release mode, nothing bad seems to happen so it is probably fine... : Crashes when trying to load CH2_weka_prepped.tif: assertion failed: self.table.is_full() ||
        (self.code_buffer.code_size() == 2 && self.next_code == 4) ||
    self.code_buffer.max_code() - Code::from(self.is_tiff) >= self.next_code
