import shutil
import subprocess
import os
import sys

shaders_path = os.path.abspath(os.path.join(os.path.realpath(__file__), os.pardir))
spirv_path = os.path.join(shaders_path, "spirv")
if os.path.exists(spirv_path):
    shutil.rmtree(spirv_path)
os.mkdir(spirv_path)

slc = []

for file in os.listdir(shaders_path):
    if not os.path.isfile(file):
        continue
    if not file.endswith(".slang"):
        continue
    if "common" in file:
        continue
    name = file.split(".")[0]
    arguments = ["slangc", file, "-target", "spirv", "-fvk-use-c-layout", "-o", "spirv/" + name + ".spv"]
    slc.append(subprocess.Popen(arguments, stdout=sys.stdout, cwd=shaders_path))

for process in slc:
    process.wait()