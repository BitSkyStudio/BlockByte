import shutil
import subprocess
import os
bbtpl_path = shutil.which("bbtpl")
asset_dir = os.path.join(os.getcwd(), "assets_generated")
print("bbtpl: " + bbtpl_path)
print("assets: " + asset_dir)
shutil.rmtree(asset_dir)
os.mkdir(asset_dir)

def generate(name, params):
    arguments = [bbtpl_path, name]
    for key, value in params.items():
        arguments.append(key + "=" + value)
    subprocess.Popen(arguments, cwd=asset_dir)
def generate_tag(name, content):
    with open(os.path.join(asset_dir, *name.split(".")) + ".txt", "w") as f:
        f.write("\n".join(content))

wood_types = [{"woodType": "oak"}]
for data in wood_types:
    generate("bb:wood_type", data)
generate_tag("#sticks", ["wood." + data["woodType"] + ".stick" for data in wood_types])
